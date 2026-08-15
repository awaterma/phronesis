# SPEC: CUE language pack and code-graph extractor

**Status:** corrective release gate, revision 2, 2026-08-12
**Target release:** 0.28.0 (blocked until this revision's evidence gate passes)
**Parent spec:** `docs/specs/SPEC-triple-store-rete.md` (revision 8)
**Affects:** `crates/phronesis-mcp/src/graph/{unit,cue,sync,mod}.rs`,
`crates/phronesis-mcp/src/syntax/{facts,cue,mod}.rs`, init, tests, catalogue,
and documentation

## Summary

Add an opt-in `cue` pack and a package-granularity CUE graph extractor. CUE
files in one directory/package are unified, so—as with Java—a file is not the
correct dependency node. Imports resolve against a repository-complete package
index derived from `cue.mod/module.cue`, and repository-local imports emit the
same `imports` relation used by every existing language.

The extractor models CUE definitions as graph elements but does not pretend
ordinary field references are function calls. CUE is a constraint language;
graph claims must use its semantics rather than force it into an imperative
language shape.

Revision 2 corrects the first implementation before release. It makes package
identity and repository-wide import resolution normative, replaces the broad
line regex with a comment/string-aware structural scan, and specifies explicit
and narrowly inferred CUE-to-data-to-Rust contracts. Existing format-5 graphs
must not be mixed with these identities; this revision requires graph format 8.

## Authority and compatibility target

Identity and resolution follow the current [CUE modules reference](https://cuelang.org/docs/reference/modules/)
and [CUE language specification](https://cuelang.org/docs/reference/spec/).
A module path comes from `cue.mod/module.cue`; a package path is that module
path plus its directory; source files in a package are unified. Major-version
suffixes are part of identity. Built-in imports are external leaves.

The compatibility baseline is the `language.version` declared by each module.
Repository-local replacements through `cue.mod/local-module.cue` require CUE
v0.17.0 or later, matching the current modules reference; earlier module
versions ignore that file and count `replacement_version_unsupported`. Missing
or newer versions produce diagnostics rather than silently selecting the
running extractor's newest semantics.

## Goals

1. Discover modules and packages without invoking `cue` or a registry.
2. Give every package a stable language-qualified identity.
3. Resolve local imports and repository-local replacements precisely.
4. Represent definitions and dependencies in the shared graph.
5. Ship a low-noise opt-in starter pack.
6. Expose build constraints, legacy directories, ambiguity, and external
   modules in diagnostics.

## Non-goals

- Implementing evaluation, unification, subsumption, or concreteness.
- Downloading OCI modules or inspecting the user module cache.
- Reproducing `cue vet`, `cue fmt`, or `cue mod tidy`.
- Treating field selection as a call graph.
- Inferring dependencies created only by CLI arguments, overlays, tags,
  injected values, or generated code.
- Modeling deprecated `cue.mod/{pkg,gen,usr}` as first-class local modules in
  v1.

## Shared graph contract

| Relation | CUE meaning |
|---|---|
| `graph_file(file)` | A tracked `.cue` file. |
| `file_type(file, kind)` | Exactly `production`, `test`, `example`, or `build`. |
| `declares_module(file, module)` | The CUE package containing the file. |
| `graph_module(module)` | The language-qualified package. |
| `element_in_file(element, file)` | A named definition/field declared in a file. |
| `element_in_module(element, module)` | The definition belongs to the unified package. |
| `imports(from, to)` | A source import resolved to a tracked repository package. |

Definitions emit revision 7's shared `graph_definition(element)` and
`defines(file, element)` facts plus both containment edges. They never emit
`defines_fn`.

`imports` is cross-language-capable by contract. CUE imports ordinarily target
CUE packages. An explicit generated-source or configuration binding may point
to another language's node; identical paths or field names never imply one.

## Identity

```text
CueUnitId       = cue:<declared module path including @major>
CuePackageId    = cue:<module path>::<relative directory>::<package>
CueDefinitionId = <CuePackageId>::<definition path>
```

Source imports use slashes and optional `:package` qualifiers; graph segments
use `::`. The package clause participates in identity because package path and
declared package name are separate axes.

```text
module:  example.com/acme/platform@v0
source:  schemas/workload/*.cue, package workload
node:    cue:example.com/acme/platform@v0::schemas::workload::workload
def:     cue:example.com/acme/platform@v0::schemas::workload::workload::#Deployment
```

Files without a module use `cue:project`; those without a package clause use a
distinct `_anonymous` segment. Neither fallback may resolve non-builtin
imports, matching CUE's module requirement.

## Discovery and package index

Discovery walks tracked `cue.mod/module.cue` files and parses, without
evaluation:

- literal `module`;
- `language.version`;
- literal dependency module paths and versions;
- local replacements in adjacent `cue.mod/local-module.cue` when the target is
  a repository-local relative directory.

Non-concrete metadata is `manifest_nonliteral`. Nested modules own their
subtrees. Duplicate module identities are `unit_id_collision`.

Build a repository-complete index before imports:

```text
packages: import path + optional package qualifier -> Vec<Owner>
Owner: unit id, package id, package name, directory, file set, constraints
```

Owners deduplicate by package id, never file. Ambiguity remains in the vector
until the import qualifier and current build list select one owner.

`@if(...)` constraints make membership depend on CLI tags. V1 indexes the
union of tracked files and records constraints. It emits an import only when
all viable selections name the same package; otherwise
`constraint_dependent`. It never adopts host tags.

## Import resolution

For each literal import:

1. Split an optional `:package` qualifier without losing `@vN`.
2. Treat CUE built-ins as `external_builtin`.
3. Resolve the longest module-path prefix from the module build list,
   including repository-local replacement.
4. Find the exact package path beneath that module.
5. Apply the qualifier.
6. Emit only when one package id remains.

No longest-package fallback is allowed: an absent subpackage is not its
ancestor. External registry modules are `import_external` and produce no
dangling nodes. Other failures distinguish `not_found`, `ambiguous`,
`constraint_dependent`, and `invalid_path`.

Imports remain stored once per source provenance when several files in a
package declare the same dependency. `Edge::fact_id` deduplicates identical
`(predicate,args)` facts during hydration; a contract test pins that behavior.
`in_cycle` therefore sees one logical package edge and means mutually dependent
CUE packages, not definitions.

## Definitions and classification

Extract top-level fields, definitions (`#Name`), hidden definitions
(`_#Name`), and attributes needed by rules. Preserve stable nested definition
segments. Dynamic labels, pattern constraints, comprehensions, and embeddings
are counted but do not receive invented identities.

Production wins on conflicting classification. `_test.cue`, `test/`, and
`tests/` are `test`; `example/` is `example`; `cue.mod/*.cue`, `_tool.cue`, and
package `tool` are `build`; all others are `production`. This is a repository
heuristic, not proof of a particular command's file selection.

## Starter pack

| Rule id | Phase | Audit | Detection |
|---|---:|---:|---|
| `audit-cue-unresolved-local-import` | deferred | no | Requires repository-index resolution rather than a per-file guess. |
| `warn-cue-conflicting-package-name` | deferred | no | Requires a repository directory/package aggregation pass. |
| `audit-cue-open-production-struct` | deferred | no | Requires a CUE parser capable of distinguishing open production definitions without heuristic false positives. |
| `warn-import-cycle` | pre | yes | Shared structural rule over CUE or mixed-language SCCs. |

These two named predicates are syntax facts, not graph relations. The
open-struct message must say openness may be intentional. Formatting,
concreteness, validation, and unused imports belong to the official toolchain
and should enter Phronesis as outcomes, not duplicate rules.

## Data-language interoperability

CUE can load JSON and YAML through CLI/package selection, but `.cue` source
does not intrinsically name every data file evaluated with it. Proximity does
not create edges. Two safe mechanisms are allowed:

1. A future toolchain trace asserts observed, provenance-tagged dependencies
   for a concrete `cue vet/export` invocation.
2. `.phronesis/graph.toml` explicitly binds a CUE definition to one tracked
   YAML/JSON artifact and, optionally, one Rust consumer type. Configuration
   is authoritative over inference and uses exact graph identities:

```toml
[[generated_artifacts]]
producer = "cue:example.game::cue::export::export"
artifact = "config/manifest.yaml"
consumer = "rust:example-app::config::manifest_types::GameManifest"
```

`producer` and `consumer` are exact existing graph IDs; `consumer` is
optional. `artifact` must resolve to exactly one tracked graph module. JSON
and YAML artifacts additionally receive key-level analysis; executable
configuration such as Lua receives only module-level flow edges. Missing,
invalid, or ambiguous references are diagnostics
(`generated_artifact_diagnostic(kind, reference)`) and emit no guessed edge.
Unresolved or ambiguous CUE imports similarly emit
`cue_import_diagnostic(file, import_path, kind)` while built-ins remain
external leaves with no edge or diagnostic. Identical explicit and inferred evidence
deduplicates.

Valid bindings use the shared relations below rather than overloading
`imports`:

| Relation | Meaning |
|---|---|
| `generates(producer, artifact_module)` | A CUE definition produces the bound data document. |
| `consumes_data(consumer, artifact_module)` | An exact indexed consumer consumes the bound data document. Explicit consumers support every indexed language pack. |
| `deserializes(consumer_type, artifact_module)` | Stronger Rust/Serde-specific evidence for a consuming `Deserialize` type. |
| `data_flows_to(artifact_module, consumer)` | Derived forward navigation from Config into a consumer in any indexed language. |
| `data_key(artifact_module, pointer)` | A top-level JSON-pointer-style key in the document. |
| `serde_field(type, field, wire_name)` | A Rust `Deserialize` field and accepted wire name. |
| `emits_key(cue_definition, artifact_module, pointer)` | A producer definition supplies a bound key. |
| `maps_data_key(artifact_module, pointer, rust_field)` | A key is accepted by the consumer field. |
| `unconsumed_data_key(artifact_module, pointer, consumer_type)` | A bound key would be silently ignored. |

The artifact module is the existing YAML/JSON document module emitted by its
language extractor (`yaml:<repo-relative-path>` or
`json:<repo-relative-path>`); it must also have `graph_module` and
`declares_module` evidence. Rust `Deserialize` structs are named with the
same package/target/module prefix as functions plus the item name, and emit
`graph_definition`, `defines`, and containment facts. A Rust field identity is
`<type>::<field>`; this exact identity is the third `maps_data_key` argument.

Bound documents index top-level keys as RFC 6901 JSON pointers: prefix `/`,
then replace `~` with `~0` and `/` with `~1`. Only one-segment, top-level
pointers are emitted in revision 8; producer and consumer mapping use the same
bounded set. Rust
extraction indexes `Deserialize` structs and fields, including `rename`,
container `rename_all`, aliases, `flatten`, and `deny_unknown_fields`.
`unconsumed_data_key` is emitted only when no field accepts the key and the
consumer has neither `flatten` nor `deny_unknown_fields`; it describes a
static silent-drop risk, not proof of runtime behavior.

Automatic producer inference is limited to one Rust function that invokes
`Command::new("cue")`, passes literal arguments containing `export`, a tracked
`.cue` input, and YAML/JSON output selection, and writes that command's stdout
to a tracked artifact path expressed as a string literal, `concat!` of string
literals, or a same-function immutable `let` initialized by either form.
Command arguments follow the same literal forms and may be supplied by
`arg`/`args` calls within that function. Consumer inference similarly
requires a statically resolved artifact path flowing to `serde_yaml` or
`serde_json` deserialization with a resolvable target type. Dynamic commands,
paths, shell strings, and cross-function dataflow remain unbound and are
counted, never guessed.

## Invalidation

Every `.cue` edit re-extracts all CUE files in the edited package after building
a complete repository package index; stored `src` remains the physical source
file, preserving the parent spec's file-provenance contract. Edits to module
metadata, `.phronesis/graph.toml`, a package clause, or an `@if` constraint,
and every add/delete/rename, trigger a full graph rebuild in revision 8. This
deliberately favors correctness over a new partial-rebuild mechanism. A
single-file patch must never resolve against an obsolete package index. The
coordinated release writes graph format 8 so older file identities and seams cannot
survive beside package identities.

## Pack mechanics

Add `Pack::Cue`, parse `cue`, update exhaustive labels, `Pack::ALL`, dispatch,
tracked-file discovery, CLI messages, catalogue, and pack docs. `base` does not
include it. Module metadata participates in freshness despite living below
`cue.mod`.

## Testing and evidence gate

- Manifests: modern modules, `@vN`, dependencies, local replacements, nested
  modules, malformed/nonliteral values, and collisions.
- Identity: many files per package, qualifiers, anonymous files, multiple
  package names in one directory, and constrained membership.
- Imports: builtin, same module, local replacement, external, absent
  subpackage, ambiguous owner, alias, and malformed path.
- Syntax: definitions, dynamic labels, comprehensions, embeddings,
  comments/strings, and malformed input.
- Integration: rebuild, invalidation, query, audit, CUE cycles, and an
  explicitly bound CUE-to-YAML/JSON-to-Rust chain.
- Data seams: malformed/missing/ambiguous bindings, explicit-over-inferred
  precedence, inference bounds, Serde rename/rename_all/alias/flatten/
  deny_unknown_fields, deduplication, mapped keys, and silent-drop findings.
- At least two real modules, one constrained. Report counts by outcome,
  package/definition totals, timings, and manual cycle inspection.
- Official CUE checks on fixtures where available plus workspace format,
  tests, clippy, and diff checks.
- Field-test a temporary copy of the downstream consumer while excluding `.git`, `.phronesis`,
  and build output. Record the source worktree status before and after. The CUE
  base-edge count must fall by at least 50% from 19,161; every emitted local
  import must target a real package node; block imports must appear; `cue:list`
  must not dangle; and the manifest chain must expose producer definitions,
  artifact keys, mapped `GameManifest` fields, and known unconsumed keys.

## Risks and honest limits

CUE instances depend on tags, file selection, overlays, and unification.
Static extraction can establish a declared import and possible repository
owner; it cannot prove the final value or participation in a command. Findings
must state that limit and expose ambiguity rather than select a convenient
world.

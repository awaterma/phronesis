# SPEC: JSON language pack and code-graph extractor

**Status:** draft, revision 1, 2026-08-11
**Target release:** a future MINOR release
**Parent spec:** `docs/specs/SPEC-triple-store-rete.md` (revision 7)
**Affects:** `crates/phronesis-mcp/src/graph/{unit,json,sync,mod}.rs`,
`crates/phronesis-mcp/src/syntax/{facts,json,mod}.rs`, init, tests, catalogue,
and documentation

## Summary

Add an opt-in `json` pack and a document-level JSON graph extractor. Plain
JSON has no import or module system, so every tracked document is a graph node
but emits no dependency merely because a string resembles a path. Recognized
document dialects—initially JSON Schema—may define static reference semantics;
repository-local `$ref` values then emit ordinary `imports` edges.

This distinction is the design's core. The graph can include configuration
alongside code without inventing relationships from arbitrary keys. JSON and
YAML representations of the same schema dialect share resolution semantics but
retain language-qualified document identities.

## Authority and compatibility target

Generic syntax follows [RFC 8259](https://www.rfc-editor.org/rfc/rfc8259).
Schema identity and references follow [JSON Schema Draft 2020-12 Core](https://json-schema.org/draft/2020-12/json-schema-core.html),
while recognized older `$schema` dialect URIs select their own compatibility
rules. A document without a recognized dialect remains generic JSON.

JSON itself defines objects, arrays, numbers, strings, booleans, and null; it
does not define imports. `$ref` is not special outside a dialect that assigns
it semantics.

## Goals

1. Track `.json` and explicitly configured JSON-like files in the shared graph.
2. Preserve strict parse failures and duplicate-key evidence instead of
   normalizing it away.
3. Identify JSON Schema resources, anchors, and repository-local references.
4. Resolve JSON-to-JSON and JSON-to-YAML schema dependencies with one URI
   algorithm.
5. Ship syntax-safe, low-noise starter rules.
6. Keep arbitrary application JSON free of guessed edges.

## Non-goals

- Assigning universal semantics to arbitrary keys such as `include`, `path`,
  `extends`, or `dependencies`.
- Validating instances against schemas in v1.
- Implementing every JSON-based ecosystem manifest.
- Fetching HTTP(S) schemas or resolving outside the repository.
- Treating JSONC/JSON5 as JSON. They require explicit future language modes.
- Modeling JSON Pointer navigation as graph edges between every field.

## Shared graph contract

| Relation | JSON meaning |
|---|---|
| `graph_file(file)` | A tracked JSON document. |
| `file_type(file, kind)` | Exactly `production`, `test`, `example`, or `build`; data/manifest are descriptive roles only. |
| `declares_module(file, module)` | The document resource node. |
| `graph_module(module)` | A language-qualified document/resource. |
| `element_in_file(element, file)` | A schema resource/anchor physically in the file. |
| `element_in_module(element, module)` | Resource containment. |
| `imports(from, to)` | A recognized dialect reference resolved to a tracked repository resource. |

Schema resources and anchors emit revision 7's `graph_definition` and `defines`
facts; they never abuse `defines_fn`.

`imports` may cross language prefixes. A JSON Schema `$ref: "common.yaml"`
may resolve to `yaml:<unit>::common.yaml::doc:0`; the reverse is equally valid.
The existing SCC derivation can therefore reveal a genuine reference cycle
across JSON and YAML. Fragment-only references remain inside one resource and
do not emit a module-level self-edge.

## Identity and ownership

```text
JsonUnitId     = json:<nearest project unit>
JsonDocumentId = json:<unit>::<repo-relative path>
JsonResourceId = <JsonDocumentId>::resource:<canonical absolute $id>
JsonAnchorId   = <JsonResourceId>::anchor:<name>
```

The default unit is the nearest recognized repository package/root already
discovered by Phronesis; if several language units claim the path, use the
innermost root but retain the `json:` prefix. A JSON-native unit claim wins a
same-prefix collision; borrowed files fall through to the next-nearest owner
or `json:project`. With no owner, use `json:project`. The repo-relative path is
always present, so two files cannot merge because they declare the same `$id`.

`$id` creates a resource identity alias used for URI resolution; it does not
replace the physical document node. Duplicate canonical `$id` values form an
ambiguous alias and are never resolved by traversal order.

File classification is path and recognized-manifest based. Fixtures under
`test`, `tests`, or `fixtures` are `test`; `examples` are `example`; known
manifests and schema catalogues are `build`; otherwise `production`.
Production wins if multiple project owners disagree.

## Parsing and lossless evidence

Use a parser that reports byte spans and preserves duplicate object members.
`serde_json::Value` alone is insufficient because it discards earlier
duplicates and makes a dangerous document appear unambiguous. The parser must
reject comments, trailing commas, single-quoted strings, NaN, and Infinity in
strict JSON mode.

Malformed documents set `parse_failed`; incremental sync preserves the old
edges and leaves freshness stale, following the parent spec. Size and nesting
limits protect hook latency. Limit failures are distinct diagnostics.

## Dialect classification

V1 recognizes JSON Schema only when one of these holds:

1. root `$schema` is a supported JSON Schema meta-schema URI;
2. `.phronesis/graph.toml` declares the file/directory dialect;
3. a recognized schema catalogue explicitly registers it.

The presence of `$ref`, `$defs`, `properties`, or a `.schema.json` suffix alone
is insufficient to choose semantics. Unknown `$schema` values are
`dialect_unsupported` and emit no imports.

Future adapters for OpenAPI, AsyncAPI, npm, TypeScript config, and other JSON
formats must specify their own keys and resolution roots. They may reuse the
document parser and identity, not silently expand the base dialect.

## JSON Schema indexing and reference resolution

Build a repository-complete resource index before resolving references:

```text
physical path -> document node
canonical absolute $id -> Vec<ResourceOwner>
resource + $anchor/$dynamicAnchor -> anchor element
```

Walk schema resources according to the selected dialect. Resolve a reference
against the current resource's base URI. Then:

- fragment-only JSON Pointer or anchor: internal; validate/count but emit no
  module edge;
- relative file URI: normalize against the current physical/base URI and
  require the target to stay in the repository;
- absolute URI matching one indexed `$id`: resolve to that owner;
- `http`/`https` not matching a local `$id`: external, no fetch and no edge;
- unsupported schemes: diagnostic, no edge.

A URI that resolves to a YAML file is handed to the YAML schema-resource
index. Resolution succeeds only if the YAML document has a recognized JSON
Schema dialect and one unambiguous target resource. Percent decoding,
dot-segment removal, URI fragments, `$anchor`, `$dynamicAnchor`, and nested
`$id` scope require dedicated fixtures.

`$dynamicRef` emits a dependency to the statically identified resource but is
tagged/counted `dynamic_scope`; the edge proves a resource dependency, not the
runtime-selected anchor. Older draft semantics are implemented only when their
meta-schema selects them.

## Starter pack

| Rule id | Phase | Audit | Detection |
|---|---:|---:|---|
| `block-json-duplicate-key` | deferred | no | The strict pre-parse rejects duplicate members before `serde_json::Value`, but a blocking rule requires stable member names and source spans rather than a generic parse failure. |
| `warn-json-schema-unresolved-local-ref` | deferred | no | Requires repository-index resolution rather than treating every relative reference as unresolved. |
| `audit-json-schema-unknown-dialect` | audit | yes | `$schema` exists but dialect is unsupported. |
| `warn-import-cycle` | pre | yes | Shared structural rule over schema resources, including JSON/YAML cycles. |

Duplicate keys are a block candidate because consumers disagree or silently
choose one value, but it must begin as warn during a corpus phase if the parser
cannot prove exact RFC object-member boundaries. Generic JSON receives no
schema rules.

## Multilingual integration

JSON does not automatically depend on code that deserializes it. The static
graph may gain cross-language edges only from explicit evidence:

- a Helm `.Files.Get` or chart value source can import a JSON document;
- a CUE graph binding or traced command can import it;
- JSON/YAML Schema references can cross representation;
- project configuration can bind a generated JSON artifact to its producer.

Filename similarity, shared field names, and directory proximity never create
edges. Explicit bindings carry configuration provenance so compaction and
explanation can name their source.

## Invalidation

Ordinary edits compact the document's provenance. Any change to `$id`,
`$schema`, an anchor, a schema catalogue, or graph configuration can alter
references from other documents and triggers full rebuild unless a reverse URI
index supports targeted invalidation. Add/delete/rename also rebuilds.

The coordinated release writes graph format 5 for revision 7's shared
definition, multi-module-file, and multilingual-import contracts.

## Pack mechanics

Add `Pack::Json`; do not alias `jsonc` or `json5`. Register file discovery,
parsing, graph dispatch, catalogue, CLI pack lists, and documentation. Known
JSON manifests already consumed by other unit discovery remain one physical
file: discovery metadata and the JSON document node may coexist without
duplicating `graph_file` facts.

## Testing and evidence gate

- RFC syntax: every value type, duplicate keys, malformed escapes/numbers,
  comments, trailing commas, BOM policy, depth/size limits, and UTF-8 failure.
- Dialect selection: supported/old/unknown/missing `$schema`, configured
  dialect, and misleading `$ref` in generic JSON.
- URI resolution: nested `$id`, pointers, anchors, dynamic anchors, relative
  paths, percent escapes, absolute local ids, external URIs, unsupported
  schemes, traversal, ambiguity, and JSON-to-YAML references.
- Incremental invalidation when resource identities and files change.
- Mixed graph integration, including a genuine JSON/YAML schema cycle and a
  negative case where arbitrary path-looking strings emit nothing.
- Real corpora: one schema suite and one generic JSON-heavy repository. Report
  documents by dialect, refs by outcome, duplicate keys, edges, timings, and
  manual review of all warnings/cycles.
- Workspace format, tests, clippy, and diff checks.

## Risks and honest limits

JSON is a carrier for many unrelated languages. Incorrect dialect inference is
the primary risk because it invents edges that look authoritative. V1 chooses
explicit dialect evidence over coverage. A clean generic-JSON graph means
"tracked, with no standard dependency semantics," not "independent of the
rest of the repository."

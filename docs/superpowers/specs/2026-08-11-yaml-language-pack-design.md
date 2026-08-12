# SPEC: YAML language pack and code-graph extractor

**Status:** draft, revision 1, 2026-08-11
**Target release:** a future MINOR release
**Parent spec:** `docs/specs/SPEC-triple-store-rete.md` (revision 7)
**Sibling specs:** JSON and Helm 3 language-pack designs dated 2026-08-11
**Affects:** `crates/phronesis-mcp/src/graph/{unit,yaml,sync,mod}.rs`,
`crates/phronesis-mcp/src/syntax/{facts,yaml,mod}.rs`, init, tests, catalogue,
and documentation

## Summary

Add an opt-in `yaml` pack and a document-level YAML graph extractor. Generic
YAML has anchors and aliases within a document but no standard cross-file
import. It therefore produces tracked document nodes and intra-document
elements, but no dependency from arbitrary scalar values. Recognized dialects,
initially JSON Schema expressed as YAML, may emit repository-local `imports`
edges under their own semantics.

Helm templates are not parsed by this extractor even though their filenames
often end in `.yaml`; the Helm 3 extractor owns files beneath a valid chart's
`templates/` directory. Rendered YAML and template source are different
languages and must not be conflated.

## Authority and compatibility target

Generic syntax follows [YAML 1.2.2](https://yaml.org/spec/1.2.2/). The default
schema is the YAML Core schema unless project configuration selects JSON or
Failsafe. Anchors and aliases are scoped to one document. A stream may contain
multiple documents. Merge keys are not part of YAML 1.2.2 Core and are treated
as an opt-in legacy feature, not silently normalized.

JSON Schema documents encoded as YAML follow the dialect selected by their
`$schema` URI and the JSON Schema rules in the sibling JSON spec.

## Goals

1. Track YAML streams and individual documents in the shared graph.
2. Preserve duplicate keys, tags, anchors, aliases, directives, and spans.
3. Share JSON Schema resource/reference resolution across JSON and YAML.
4. Exclude Helm template source from generic YAML parsing.
5. Ship conservative syntax and schema-reference rules.
6. Never infer a cross-file dependency from a domain-specific key without a
   selected dialect adapter.

## Non-goals

- Implementing Kubernetes, GitHub Actions, Compose, Ansible, CloudFormation,
  or every YAML-hosted language in the base pack.
- Rendering Helm or other template delimiters as YAML.
- Expanding merge keys by default.
- Fetching remote tags or schema resources.
- Equating aliases with cross-file imports.
- Validating a domain document against its schema in v1.

## Ownership precedence

A file is assigned to exactly one syntax extractor:

1. A file beneath `templates/` of a valid Helm chart, including `.yaml`,
   `.yml`, `.tpl`, and `NOTES.txt`, belongs to `helm3`.
2. `Chart.yaml`, `values.yaml`, and `values.schema.json` remain data documents
   owned by YAML/JSON extraction and are linked to the chart by Helm edges.
3. Other `.yaml` / `.yml` files belong to YAML.

This precedence must be centralized in graph dispatch. Running YAML first and
Helm second would transiently compact one file under two incompatible
identities.

## Shared graph contract

| Relation | YAML meaning |
|---|---|
| `graph_file(file)` | A tracked YAML stream. |
| `file_type(file, kind)` | Exactly `production`, `test`, `example`, or `build`; config/manifest are descriptive roles only. |
| `declares_module(file, module)` | One relation per document node in the stream. |
| `graph_module(module)` | A language-qualified YAML document. |
| `element_in_file(element, file)` | Anchor/schema resource physically in the stream. |
| `element_in_module(element, module)` | Element belongs to a document. |
| `imports(from, to)` | A selected dialect's static reference to a tracked resource. |

Schema resources and anchors emit revision 7's shared `graph_definition` and
`defines` relations; they never emit `defines_fn`.

An alias references an anchor in the same YAML document. Validate and count it,
but do not emit `imports(module, module)`. Alias cycles are data-graph cycles,
not module dependency cycles, and do not belong in `in_cycle`.

## Identity

```text
YamlUnitId     = yaml:<nearest project unit>
YamlStreamId   = yaml:<unit>::<repo-relative path>
YamlDocumentId = <YamlStreamId>::doc:<zero-based source ordinal>
YamlAnchorId   = <YamlDocumentId>::anchor:<name>
```

Document ordinals are stable only while earlier documents are unchanged. They
are acceptable derived identities because a stream edit replaces every edge
with that file's provenance. Cross-file dialect references target a resource
`$id` alias where available; they must not persist an ordinal guessed from a
multi-document target unless the dialect explicitly defines fragment
selection.

Use the innermost existing project unit with the `yaml:` prefix, falling back
to `yaml:project`. Physical path remains in every stream identity, preventing
same-named units from merging data documents.

## Parser requirements

Use an event/CST parser that:

- supports YAML 1.2 streams and reports spans;
- preserves duplicate mapping keys before construction;
- exposes tag spelling, anchors, aliases, directives, scalar style, and
  document boundaries;
- can impose byte, token, alias-expansion, and nesting limits;
- does not resolve or expand aliases merely to extract graph facts.

`serde_norway::Value` is not sufficient by itself because construction can
erase duplicate-key and source-style evidence. Tree-sitter may provide the CST
while a YAML parser validates semantics; the implementation spec must pin both
versions and define disagreement behavior. If either parser rejects the stream,
set `parse_failed`, preserve prior edges, and leave freshness stale.

## Generic YAML extraction

Emit one module per document, plus anchors as elements. Check:

- duplicate explicit mapping keys after the selected schema resolves scalar
  equality;
- duplicate anchor names within a document;
- aliases whose anchor has not appeared earlier in that document;
- aliases crossing document boundaries;
- unsupported/custom tags;
- legacy merge-key usage.

The extractor does not expand aliases. Recursive aliases are counted, not
walked. Diagnostics distinguish syntactic invalidity, schema-dependent key
equality, resource limits, and unsupported tags.

No generic scalar emits `imports`, including values beneath keys named
`include`, `extends`, `$ref`, `file`, `path`, or `source`. A dialect must own
the key.

## Dialect adapters

V1 includes JSON Schema in YAML when root `$schema` selects a supported
dialect or `.phronesis/graph.toml` explicitly classifies the file. It shares
the JSON sibling spec's URI/resource index. YAML-to-JSON and YAML-to-YAML
repository refs emit `imports`; fragment-only refs do not.

A stream with multiple documents is eligible only when the configuration
selects the target document(s) or each schema document independently declares
a dialect. Ambiguous references to a multi-document physical file are counted
and skipped.

Future adapters (Kubernetes, Actions, Compose, Ansible) require separate specs
covering versions, key semantics, and resolution. Filename convention alone is
insufficient.

## Classification

Files under test/fixture paths are `test`; examples are `example`; recognized
workflow/build manifests and `Chart.yaml` are `build`; others are
`production`. Helm template-source precedence applies first. Production wins
when project ownership claims conflict.

## Starter pack

| Rule id | Phase | Audit | Detection |
|---|---:|---:|---|
| `block-yaml-duplicate-key` | pre | yes | Exact duplicate mapping key under selected schema. |
| `block-yaml-invalid-alias` | pre | yes | Undefined or cross-document alias. |
| `warn-yaml-legacy-merge-key` | post | yes | `<<` merge key without explicit legacy opt-in. |
| `warn-yaml-schema-unresolved-local-ref` | deferred | no | Requires repository-index resolution rather than a per-file guess. |
| `warn-import-cycle` | pre | yes | Shared structural rule over JSON/YAML schema resources. |

Blocks require parser-level precision and begin as warnings during corpus
validation. Custom tags are audit-only by default because many ecosystems use
them legitimately.

## Multilingual integration

Safe cross-language edges include:

- JSON Schema references across JSON and YAML representations;
- Helm chart/template nodes importing `values.yaml`, schema, or `.Files.Get`
  data files;
- explicit/traced CUE inputs;
- configured generated-artifact bindings.

YAML documents do not automatically import the code that consumes them or the
schema that might validate them. Bindings carry provenance and must resolve to
one tracked node.

## Invalidation

Any ordinary stream edit replaces all its document edges. Changes to anchors
are file-local; changes to a schema `$id`, `$schema`, catalogue, graph config,
or file path may alter other documents' resolution and trigger full rebuild.
File add/delete/rename also rebuilds. Ownership changes involving `Chart.yaml`
or a `templates/` boundary rebuild YAML and Helm together.

The coordinated release writes graph format 5 for revision 7's shared
definition, multi-module-file, and multilingual-import contracts.

## Pack mechanics

Add `Pack::Yaml` with alias `yml`, but serialize/document `yaml`. Register
`.yaml` and `.yml`, excluding Helm-owned template source. Update catalogue,
CLI lists, docs, audit adapters, and rebuild/status enumeration. `base` remains
language-agnostic.

## Testing and evidence gate

- YAML 1.2 streams: block/flow collections, scalar forms, directives, tags,
  anchors/aliases, multiple documents, malformed syntax, duplicate keys,
  merge keys, BOM/encoding, depth/size/alias limits.
- Schema-dependent equality cases such as boolean-looking and numeric keys.
- Dialect selection and every JSON Schema URI case from the JSON spec,
  including cross-format references and multi-document ambiguity.
- Ownership precedence for Helm templates, Chart/values/schema documents, and
  chart creation/deletion.
- Negative tests proving arbitrary path-like scalars produce no edges.
- Real corpora: generic configuration, multi-document Kubernetes YAML (generic
  in v1), and YAML JSON Schema. Report documents, dialects, parse failures,
  refs, warnings, edges, and timings.
- Workspace format, tests, clippy, and diff checks.

## Risks and honest limits

YAML is a syntax substrate for many domain languages. Parsing it correctly
does not grant knowledge of those languages. The pack reports generic YAML
integrity and explicitly selected dialect semantics; it must not describe a
domain dependency as absent merely because no adapter was enabled.

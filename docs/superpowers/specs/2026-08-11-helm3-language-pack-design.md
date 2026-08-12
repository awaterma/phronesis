# SPEC: Helm 3 language pack and code-graph extractor

**Status:** draft, revision 1, 2026-08-11
**Target release:** a future MINOR release
**Parent spec:** `docs/specs/SPEC-triple-store-rete.md` (revision 7)
**Sibling specs:** JSON and YAML language-pack designs dated 2026-08-11
**Affects:** `crates/phronesis-mcp/src/graph/{unit,helm3,sync,mod}.rs`,
`crates/phronesis-mcp/src/syntax/{facts,helm3,mod}.rs`, init, tests, catalogue,
and documentation

## Summary

Add an opt-in `helm3` pack and a chart-aware Helm 3 extractor. Helm template
source is Go template language embedded in files that often have YAML names;
it is not valid YAML before rendering. A valid chart boundary therefore owns
its `templates/` source, while `Chart.yaml`, `values.yaml`,
`values.schema.json`, and files read through `.Files` remain YAML/JSON document
nodes connected through cross-language `imports` edges.

The extractor models charts, template files, named templates, includes,
subchart dependencies, value inputs, and statically named chart files in the
same graph used by application code. It never renders a chart in the hook.

## Authority and compatibility target

Target Helm 3 behavior documented by the [Helm chart format](https://helm.sh/docs/topics/charts/),
[named templates guide](https://helm.sh/docs/v3/chart_template_guide/named_templates/),
and [chart template guide](https://helm.sh/docs/chart_template_guide/).
Helm templates use Go `text/template` plus Sprig and Helm functions. Named
template names are global across the parent chart and subcharts, and load order
can select the last duplicate definition; that ambiguity must remain visible.

This pack is named `helm3` so a later Helm 4 compatibility decision does not
silently change existing rules. The parser may support common syntax shared by
both versions, but fixtures and claims are Helm 3.

## Goals

1. Discover local charts from valid `Chart.yaml` files and chart boundaries.
2. Parse template actions without treating surrounding YAML/text as YAML AST.
3. Resolve named template calls, chart dependencies, `.Files` reads, and
   schema/default-value inputs.
4. Emit cross-language edges to YAML and JSON documents in the shared graph.
5. Detect deterministic, high-value chart defects without invoking a cluster.
6. Keep dynamic template behavior counted and explicit.

## Non-goals

- Rendering templates or reproducing `helm lint`, install, upgrade, or
  dependency download.
- Contacting chart repositories, OCI registries, or Kubernetes clusters.
- Evaluating Go templates, Sprig, `tpl`, `lookup`, capabilities, or values.
- Parsing rendered Kubernetes resources in v1.
- Inferring all possible value overrides.
- Treating Helm 4 behavior as covered.

## Ownership and phase order

Discovery is repository-wide and precedes YAML dispatch:

1. Parse candidate `Chart.yaml` files as strict chart metadata.
2. Establish nested chart roots and local `charts/` ownership.
3. Assign files beneath each owned `templates/` directory to Helm 3.
4. Index named template definitions globally within each render set.
5. Dispatch remaining YAML/JSON data files to their language extractors.
6. Resolve template, file, values, schema, and dependency edges against the
   completed multilingual index.

A directory named `templates` without a valid chart ancestor is ordinary
YAML/text. A nested valid chart starts a new chart unit; the parent sees it as
a dependency only when `Chart.yaml` or vendored-chart metadata claims it.

## Shared graph contract

| Relation | Helm 3 meaning |
|---|---|
| `graph_file(file)` | Tracked Helm template source or chart metadata. |
| `file_type(file, kind)` | Exactly `production`, `test`, `example`, or `build`; template/metadata are descriptive roles only. |
| `declares_module(file, module)` | Template file/module; metadata may also declare the chart module. |
| `graph_module(module)` | Chart, template file, named-template module, or synthetic values node. |
| `element_in_file(element, file)` | Named template physically defined in a file. |
| `element_in_module(element, module)` | Named template belongs to a chart render set. |
| `imports(from, to)` | Static include/template/file/schema/value/chart dependency. |

Named templates are callable, but they are not ordinary language functions.
They emit revision 7's `graph_definition` and `defines`, not `defines_fn` or
`tested_by`. Dynamic Helm calls use syntax facts rather than repurposing the
function-only `calls_api` relation.

Every cross-language dependency uses `imports`. Examples:

```text
helm3:charts/app::template::deployment.yaml
  -> helm3:charts/app::named::app.labels

helm3:charts/app::template::configmap.yaml
  -> yaml:charts/app::files/config.yaml::doc:0

helm3:charts/app::chart
  -> json:charts/app::values.schema.json
```

This lets query and SCC derivation traverse real mixed-language dependencies.
It does not mean every edge can form a runtime import cycle; `in_cycle` wording
must say "static build/evaluation dependency cycle" for mixed graphs.

## Identity

```text
HelmChartId    = helm3:<repo-relative chart root>::chart
HelmTemplateId = helm3:<repo-relative chart root>::template::<relative path>
HelmNamedId    = helm3:<repo-relative chart root>::named::<literal name>
HelmValuesId   = helm3:<repo-relative chart root>::values
```

Physical chart root, not `Chart.yaml`'s `name` or `version`, anchors identity.
Names can collide across vendored charts and versions change routinely; path
identity remains stable across a version bump. The declared name/version are
metadata used for dependency resolution and diagnostics.

Named templates execute in a render set where parent and subchart definitions
share a global namespace. `HelmNamedId` records physical ownership, while a
separate resolver index maps literal template name to `Vec<Owner>` in the
render set. Duplicate names remain ambiguous even though Helm would select one
by load order; choosing that winner would make results dependent on an
implementation detail and can hide the collision the pack should report.

## Chart discovery

Parse the Helm 3 subset of `Chart.yaml`: `apiVersion`, `name`, `version`,
`type`, and dependency entries (`name`, `alias`, `version`, `repository`,
`condition`, `tags`, `enabled`, `import-values`). Require `apiVersion: v2` for
Helm 3 chart ownership. Library charts are units and may define templates but
normally render no manifests.

Resolve a dependency only to a repository chart when exactly one of these
holds:

- a vendored chart directory/archive under `charts/` has matching declared
  name or alias and compatible literal version;
- a `file://` repository URI normalizes to a tracked chart root inside the
  repository;
- `.phronesis/graph.toml` explicitly binds it.

Remote repositories are `dependency_external`. Conditions and tags affect
whether a dependency is enabled for particular values; v1 emits the structural
edge but tags it/counts `conditional_dependency`. The edge means the chart
declares the dependency, not that every render enables it.

Archives are not unpacked by the hook. A checked-in `.tgz` is counted as an
opaque external/vendored dependency unless a future archive index is designed
with size and traversal protections.

## Template parsing and extraction

Use a Go-template-aware parser or a purpose-built action lexer plus parser. A
YAML parser and regex are both rejected. The parser must handle:

- `{{ ... }}` actions and whitespace trimming;
- quoted strings, raw strings, variables, pipelines, parentheses, comments;
- `define`, `block`, `template`, `include`, `tpl`, and `.Files` calls;
- nested `if`, `with`, and `range` scopes;
- multiple definitions per physical file;
- malformed/unclosed actions without erasing previous graph state.

Surrounding bytes are opaque output text. YAML-looking keys, documents, and
indentation in template source create no YAML graph facts.

### Named templates

Literal `define` and `block` names populate the render-set index. Literal
`template "name"` and `include "name"` resolve to one owner and emit
`imports`. Dynamic names, missing owners, and duplicate owners are separately
counted. Calls in a named definition originate from its `HelmNamedId`; calls
at file top level originate from `HelmTemplateId`.

`block` has both roles: it registers a named-template definition and emits an
`imports` edge from its containing template to the selected named-template
owner because the action renders that template (or its override).

### Values

Any `.Values` access emits one deduplicated dependency from its origin to the
synthetic `HelmValuesId`. The chart node imports `values.yaml` when present and
imports `values.schema.json` when declared/present, linking the synthetic value
contract to concrete YAML/JSON documents without claiming defaults are the
only runtime source.

Extract normalized static value paths (`.Values.image.repository`,
`index .Values "image"`) as elements/diagnostics. Dynamic indexes are counted.
V1 does not warn that a value is absent from defaults because callers may
supply it. A future rule may combine schema `required`, defaults, and guarded
access.

### Chart files

Resolve literal `.Files.Get`, `.Files.GetBytes`, `.Files.Glob`, `AsConfig`, and
`AsSecrets` paths within the chart, excluding `templates/`, files excluded by
`.helmignore`, and paths outside the chart. Exact literal gets emit imports to
the owning YAML, JSON, or generic file node. A glob emits edges to the bounded,
sorted set of current matches and records the pattern. Dynamic paths are
counted, not guessed.

V1 emits `.Files` edges only when the target is already owned by another
extractor, initially YAML or JSON. A text/binary/unowned target is counted as
`files_target_untracked` and emits no edge. A shared arbitrary-resource node is
deferred to a later parent-spec revision; the extractor never manufactures a
`yaml:` node for arbitrary content.

### Dynamic functions

Emit `helm3_tpl_call(file, origin)` and `helm3_lookup_call(file, origin)` syntax
facts. `tpl` evaluates a string as a template, so static dependencies inside it
cannot be known. `lookup` depends on cluster state and behaves differently in
offline rendering. These are review signals, not proof of a defect. Literal
file input to `tpl` still emits its `.Files` edge; nested dependencies remain
unknown.

## Starter pack

| Rule id | Phase | Audit | Detection |
|---|---:|---:|---|
| `warn-helm3-duplicate-template-name` | deferred | no | Requires chart render-set aggregation across files and dependencies. |
| `warn-helm3-unresolved-template` | deferred | no | Requires chart-index resolution across files and dependencies. |
| `warn-helm3-unresolved-file` | deferred | no | Requires chart-root, `.helmignore`, and packaged-file resolution. |
| `audit-helm3-tpl` | audit | yes | Dynamic template evaluation. |
| `audit-helm3-lookup` | audit | yes | Render depends on live cluster state. |
| `warn-import-cycle` | pre | yes | Shared structural rule over static-dependency cycles. |

No rule blocks in v1. `helm lint` findings should feed outcome/confidence
signals rather than be recreated as a large rule catalogue.

## Testing and evidence gate

- Chart discovery: valid/invalid v2 metadata, nested charts, library charts,
  aliases, file dependencies, vendored directories, archives, conditional and
  external dependencies, collisions, and traversal attempts.
- Ownership precedence proving template `.yaml` is never parsed as YAML while
  Chart/values/schema and `.Files` resources are.
- Parser corpus for action syntax, whitespace, strings, comments, pipelines,
  scopes, multiple definitions, malformed actions, and false delimiters in
  comments/quoted output.
- Named-template resolution: same file, cross-file, parent/subchart, literal,
  dynamic, missing, and duplicate.
- Values access and schema/default links without claiming missing defaults.
- `.Files` exact/glob/dynamic paths, `.helmignore`, forbidden directories,
  binary/text handling, and cross-language YAML/JSON targets.
- Integration through rebuild, incremental save, query, audit, graph status,
  and at least one genuine mixed-language dependency path.
- Real corpora: one application chart, one library/subchart suite, and one
  chart using `tpl`/`.Files`. Report every edge/counter family, render-set
  collisions, timings, and manual review of warnings and cycles.
- Compare fixtures with Helm 3 `helm lint` and `helm template` where installed;
  then workspace format, tests, clippy, and diff checks.

## Invalidation

Template edits compact locally but named-template changes can affect callers
across the render set, so derivation/resolution reruns for that chart. Changes
to Chart metadata, dependencies, `.helmignore`, values/schema files, chart
add/delete/rename, or files matched by `.Files.Glob` trigger chart-scoped or
full rebuild. V1 always performs a full rebuild when any YAML/JSON file beneath
a chart root changes; a reverse target-to-template index is deferred. This is
the correctness-first implementation despite its higher cost.

The coordinated release writes graph format 5 for revision 7's shared
definition, ownership-arbitration, and multilingual-import contracts.

## Pack mechanics

Add `Pack::Helm3`; accept `helm3` and convenience input `helm`, but document and
serialize `helm3`. Add chart-aware ownership before extension dispatch,
catalogue/CLI/docs entries, audit adapters, and rebuild/status coverage. The
pack may compose with `yaml,json`; it must work independently while still
tracking chart metadata required for Helm edges.

## Risks and honest limits

Helm is an evaluation language with values, capabilities, subcharts, dynamic
templates, optional dependencies, and cluster reads. Static extraction proves
literal references and declared structure; it does not prove rendered output.
Graph explanations must distinguish declared, conditional, dynamic, and
observed dependencies. A warning about `lookup` or `tpl` is a request for
review, not a claim that the chart is unsafe.

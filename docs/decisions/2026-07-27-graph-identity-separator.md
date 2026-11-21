---
id: graph-identity-separator
date: 2026-07-27
status: accepted
enforces: []
superseded_by: null
tags: [code-graph, identity, multi-language]
---

# Code-graph identities always separate segments with `::`

## Context

The code graph names entities `<lang>:<package>[#<target>]::<module path>`
(SPEC-triple-store-rete §4.2). When the Python extractor landed, dotted
Python-style identities looked more natural — `python:shop-api::shop.orders`
rather than `python:shop-api::shop::orders` — and the question came up of
whether the separator should be configurable per language or per project.

Two findings settled it.

**The separator is load-bearing, not cosmetic.** `graph::derive::short_name`
splits on `::` to bridge `tested_by`, which carries bare callee names, to
`defines_fn`, which carries qualified ones. With a dotted Python identity,
`short_name("python:shop-api::shop.orders.load")` returns
`"shop.orders.load"`, which never equals the `"load"` a pytest test
contributes — so **every tested Python function would report as untested**.
That is a false positive in an enforcement layer, which §4.4 identifies as the
unrecoverable direction: a missed warning is recoverable, a false "this is
broken" verdict destroys trust in the whole pack.

**`::` is the only separator that cannot occur inside the names being
joined.** Real distribution names contain dots — `zope.interface`,
`ruamel.yaml`, and namespace packages generally. Verified against a project
named `zope.interface`, the graph produces:

```
python:zope.interface::zope::interface::declarations::implementer
```

Under a dotted separator that same entity is
`python:zope.interface.zope.interface.declarations.implementer`, in which
nothing can tell where the distribution name ends and the module path begins.
No Rust or Python identifier can contain `::`; dots, hyphens and underscores
all appear in real package and function names.

## Decision

The segment separator is **`::`, fixed, in every language**, regardless of
what the source syntax uses. It is part of the graph's data model, not a
rendering of any language.

The separator is **not configurable** — not per language, not per project.

If identities ever need to read more naturally, that is a **presentation**
concern: render at the message boundary while continuing to store `::`. Even
that is deferred, because a rendered name no longer greps against
`.phronesis/graph.jsonl`, and grep-ability matters more when someone is
working out why a rule fired.

## Enforcement

- (none — this is a design invariant of the extractors, not a code shape a
  predicate can match. The regression that would catch a violation is
  `graph::python::tests::a_module_file_maps_to_its_module_path` together with
  `graph::derive::tests`, which fail if the separator and `short_name`
  disagree.)

## Consequences

- Python identities read `python:shop-api::shop::orders`, not `shop.orders`.
  Accepted cost.
- Distribution and package names containing dots round-trip safely.
- A future extractor for any language inherits the invariant: whatever the
  source writes, the graph joins with `::`. See [[graph-language-tag]] for why
  the language prefix exists alongside it.
- Making the separator configurable later would change every identity on disk,
  requiring a `GRAPH_FORMAT` bump and a rebuild — it is a migration, not a
  setting. Worse, a per-project setting could disagree with whatever wrote the
  graph, a mismatch the format stamp is not designed to detect.

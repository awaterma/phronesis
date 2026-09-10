# SPEC: Java code-graph extractor

**Status:** implemented, revision 8; Qwen reassessment PASS; workspace and corpus validation complete
**Target release:** next MINOR after 0.31.1 — new extractor, existing structural cycle rule
**Parent spec:** `docs/specs/SPEC-triple-store-rete.md` (rev 6)
**Affects:** `crates/phronesis-mcp/src/graph/{java/,unit/,sync/,mod.rs}`,
`crates/phronesis-mcp/tests/graph_structural_rules.rs` (shipped-pack verification)
**Precedent:** `docs/superpowers/specs/2026-07-31-typescript-code-graph-design.md`

> **Revision 2** replaces revision 1's resolution and discovery design, which
> adversarial review found unsound. Revision 1 derived the package index from
> file *paths* while identity came from the `package` *declaration* — two
> namespaces that disagree — and resolved imports by longest-declared-package
> prefix, which invents edges to ancestor packages when a subpackage is
> third-party. Both are corrected here. The granularity argument below is
> unchanged; it survived review.
>
> **Revision 3** fixes defects found reviewing revision 2: `packages` owners
> deduplicated by unit rather than per file (a per-file vector made every
> wildcard import ambiguous); cross-unit resolution constrained by declared
> build dependencies (index presence is not classpath visibility); Bazel
> package boundaries, unit-id definitions, `file_type` precedence, and the
> expression grammar all specified rather than assumed; ordered Maven root
> resolution; `skipped` separated from the named counters; and an Invalidation
> section, because build-metadata edits reclassify files that per-file
> compaction never revisits.
>
> **Revision 4** closes the three gaps left partially fixed: `IdentRef` is
> defined; Maven and Bazel visibility are specified as scoped transitive
> closures over the reactor/`deps` graph, approximating the real classpath in
> **both** directions (an earlier draft called this a one-sided
> under-approximation, which was wrong — ignoring `<exclusions>` and Bazel
> `visibility` over-approximates, and that is the direction that can invent a
> cycle); and manifest edits route to `rebuild()` rather than `on_save`, since
> `store::compact` replaces edges only by provenance file and cannot drop a
> file that discovery no longer owns.
>
> **Revision 5** applies a fresh-eyes review: a wildcard import may name a
> *type*, not only a package (JLS §7.5.2), which a `packages`-only lookup
> dropped silently; Bazel visibility is direct `deps` plus `exports`, not a
> transitive closure, because `strict_deps` defaults to true; indexing and
> build evaluation are given an explicit phase order, since `test_class`
> resolution needs a repository-complete index; `module-info.java` is skipped
> explicitly; and the central granularity claim is qualified to "within a
> unit," since a package split across units is two nodes.
>
> **Revision 6** applies two further independent reviews. Visibility now
> models the *compile* classpath only — Maven `runtime` scope and Bazel
> `runtime_deps` are excluded, Maven's closure follows `compile` → `compile`
> edges alone (a flat closure reached targets `javac` rejects), and Bazel
> visibility is per **file**, derived from the targets claiming it, because
> `deps` are per-target while units are per-package. Also: `import module`
> (Java 25) is recognised, with a stated source level; `Owner` carries a
> production/test compilation context, without which the same FQN in
> `src/main` and `src/test` made every such import ambiguous; the declaration
> cache is keyed on a content hash rather than `(length, mtime)`, matching
> `sync.rs:75`; `test_class` reclassifies nothing, since it names a launch
> entry point rather than a claim; `groupId` inheritance is specified; and
> `file_type` precedence is **reversed to production-wins** — an earlier draft
> argued test was safer, which was backwards, since rules exempt tests and
> mislabelling production code therefore suppresses findings.

## Revision 7 implementation reconciliation

Revision 7 reconciled revision 6 against the current repository. It corrected contradictory phase ordering,
compilation-context deduplication, content-cache promises, and source-change
invalidation. The current graph implementation lives in `unit/` and `sync/`,
and persistence now checks canonical `tested_by` targets. The Maven and Bazel
backends, cycle rule, mixed-language tests, and both corpus gates remain
required; these corrections do not reduce the implementation scope.

Revision 8 incorporates the completed Qwen review: malformed module-import
names are rejected, cached pre-fix parses are invalidated, external-parent
coordinate behavior is clarified, and concrete review fixtures are covered.
See [review and verification](../../specs/REPORT-java-qwen-review.md) for the
findings, dispositions, and limits of the model's static review.

## Summary

An additional extractor for the structural code graph, covering Java, with unit
discovery from **both Maven and Bazel**.

Java inverts TypeScript's difficulty. TypeScript's hard part was resolution:
imports name filesystem *paths* that must be probed. Java imports name
fully-qualified types, and every Java file declares its own package.

The hard part here is **granularity**. Java is the first language in the graph
whose namespace is not its file. A package spans many files, and two classes
in the same package refer to each other with no import statement. That single
fact determines the design.

Java is also the first extractor that requires a **declaration index** — a map
of every package and every declared type in the project — because Java names
cannot be classified without one. It is roughly the analogue of TypeScript's
file index, and it is why this extractor is comparable in cost to TypeScript's
rather than cheaper.

## Why granularity is the risk

A missing edge is invisible. It does not error and does not look different
from a codebase that genuinely has no such dependency — it looks like a clean
result. `imports` feeds `in_cycle`, so every dropped edge is a cycle the pack
silently fails to report. Python already shipped exactly this bug with
cross-distribution imports, found only by explicitly testing for it.

For Java, the natural-seeming choice — one module per class, matching the
other three extractors — walks straight into it:

```java
package com.example.billing;
class Charge {
    Invoice invoice;   // com.example.billing.Invoice — no import statement exists
}
```

There is no import to scan, so there is no edge to draw. This is not an edge
case: Java developers deliberately co-locate collaborating classes, so
class-level extraction would capture the sparse cross-package edges while
systematically dropping the dense intra-package ones — a graph most wrong
exactly where coupling is highest.

**Consequence:** the module is the package, not the class. That does not
mitigate the hole; it removes it. Two classes in one package are one node, so
there is nothing left to miss.

**Precisely: within a unit.** Module identity is `java:<unit>::<package>`, so
one Java package split across two units is two nodes, and same-package access
between them — which needs no import statement — draws no edge. This is the
class-level gap surviving at unit granularity. It is narrow (it needs a split
package *and* implicit cross-unit access, which JPMS forbids outright and
Maven and Bazel both discourage), and split packages already surface through
owner selection and `skipped`. But the claim is "removes the hole within a
unit," not "removes the hole," and the corpus run should report split-package
counts so the residue is measured rather than assumed small.

## Identity

One namespace serves both identity and resolution. **Nothing but a `package`
declaration may create a package name, and nothing but a type declaration may
create a type name.** File paths, source roots, and build metadata never do.

```
JavaModuleId = java:<unit>::<declared package segments>
JavaTypeId   = java:<unit>::<declared package segments>::<type segments>
```

```java
package com.example.billing;
class Charge {}
class Helpers { static class Parser {} }
```

```
module: java:myapp::com::example::billing
type:   java:myapp::com::example::billing::Charge
type:   java:myapp::com::example::billing::Helpers
type:   java:myapp::com::example::billing::Helpers::Parser
```

The default package is the unit id itself (`java:myapp`).

**`<unit>` must be stable and collision-free**, since a collision silently
merges two units' packages into one node:

| Backend | Unit id | Why not the obvious choice |
|---|---|---|
| Maven | `groupId:artifactId` | `artifactId` alone collides — two reactor modules may legally share one under different groups |
| Bazel | repo-relative package label, `//foo/bar` | the rejected per-target scheme is not a definition |
| Neither | `project` | — |

`groupId` resolution is specified, not left to the reader, because every Maven
unit id depends on it and it must also be available before
`${project.groupId}` can be interpolated anywhere:

1. If the POM declares its own `<groupId>`, that wins outright.
2. Otherwise take `<parent><groupId>`.
3. If the parent element itself omits it, walk the parent chain by
   `<relativePath>` (default `../pom.xml`) until a `groupId` is found.
4. A chain that leaves the repository, or ends without one, yields no
   `groupId`; the unit id falls back to `artifactId` alone and is counted as
   `group_id_unresolved`.

Resolve `groupId` **before** interpolation, since `${project.groupId}` may
appear in a source-root path. Two units that still produce the same id are a
discovery bug: the second is rejected and counted as `unit_id_collision`
rather than merged.

**One Bazel package normally contains several Java packages.** `//foo` may
hold `com.example.a` and `com.example.b`; unit and module are independent
axes, and nothing assumes a one-to-one mapping.

Functions keep full precision, because `defines_fn` names the function:
`java:myapp::com::example::billing::Charge::charge`. Choosing package-level
*modules* costs class-level *edges*, not class-level *identity*.

A `.java` file no manifest claims falls back to `java:project`. Segments join
with `::` in every language; see
`.phronesis/wiki/decisions/2026-07-27-graph-identity-separator.md`.

### What package granularity claims

`in_cycle` runs only over `imports`, so package-level modules make the cycle
rule a package-level claim. It is coarser than a class-level cycle and can
fire where no class-level cycle exists: `billing.Charge → order.Order` plus
`order.Shipment → billing.Invoice` is a `billing` ⇄ `order` cycle, though
those four classes form a chain that never closes.

This is intended, and recorded so the rule's wording stays honest. The rule
claims **"these two packages are mutually dependent — neither can be extracted
without the other,"** not "there is a circular reference between classes." The
first is what jdepend and ArchUnit report and what Java architects act on.

## Java parser provenance

Use the published `tree-sitter-java-orchard` 0.5.8 dependency, pinned exactly
because extractor behavior depends on its node shapes. Versions 0.5.9 through
0.5.15 reject U+212A (the Kelvin sign) inside string literals in two Bazel
corpus files; the Unicode-string regression must pass before lifting this pin. The maintained fork
includes qualified record patterns such as `Outer.Item`, which the original
`tree-sitter-java` 0.23.5 release rejected. Pattern-bound names participate in
receiver shadowing so they cannot be mistaken for static type references.

The dependency supplies and compiles its generated parser; this repository
carries no generated Java C parser, custom build script, or unsafe language
loader. No JVM, Node, or Tree-sitter CLI is required to use the extractor.
Orchard represents `static` as a named `modifier` node. Module-import names
are explicitly validated so reserved words cannot become type-import evidence.
Malformed syntax still produces parse-failed inputs.

See [the crate](https://crates.io/crates/tree-sitter-java-orchard/0.5.8)
and [upstream discussion](https://github.com/tree-sitter/tree-sitter-java/pull/231).

## The declaration index

Built during discovery, keyed on dotted names:

- `packages: BTreeMap<String, Vec<Owner>>` — `com.example.billing` → owners
- `types: BTreeMap<String, Vec<Owner>>` — `com.example.billing.Helpers.Parser`
  → owners

**The two maps are independent namespaces**, and one dotted string may legally
key both — a package `com.example.Order` and a class `Order` in package
`com.example` produce the same text. Nothing disambiguates them by inspection,
which is why every resolution step names the map it queries rather than
searching for a match. The one step that consults both, the wildcard, does so
in a stated order.

An `Owner` carries `unit_id`, `module_id`, a compilation context
(`production` or `test`), and — in `types` only — `file`.
**Values are vectors, not single owners**, because split packages (the same
package declared in two Maven modules or two Bazel packages) are legal in
source trees. Discovery preserves the ambiguity rather than resolving it by
traversal order.

**`packages` owners are deduplicated by `(unit_id, module_id, context)`, never
per file.** A package normally spans many files, so a per-file owner vector would
make *every* wildcard import ambiguous under owner selection — a package of
three files would present three candidates and be skipped. Implementations retain each package owner's contributing files until
visibility filtering has completed, because Bazel visibility is target-based
even within a unit. Deduplication then uses the key above; a visible package
with three source files must still be one candidate.

### Phase order

Indexing and build evaluation are mutually dependent — the index needs unit
ids from build metadata, and Bazel's `test_class` needs the index — so the
order is fixed and must not be interleaved per directory:

1. **Ownership.** Walk manifests; assign every `.java` file a `unit_id` and a
   provisional classification. A `java_test` carrying `test_class` and no
   `srcs` records a *deferred* reference; it classifies nothing yet.
2. **Indexing.** Index all declarations across the whole repository, using the
   unit ids from phase 1.
3. **Entry-point diagnostics.** Resolve each recorded `test_class` against the
   completed index for diagnostics only; never reclassify its file.

Phase 2 must complete repository-wide before phase 3 begins. Evaluating one
Bazel package at a time would leave a `test_class` naming a class in a
not-yet-indexed package unresolvable, and the result would depend on
directory traversal order.

**A deferred `test_class` reclassifies nothing.** `test_class` names the class
Bazel *launches*, not a source the test target claims — it is routinely
satisfied through `runtime_deps`, a library target, or generated code. Using
it to reclassify would mark a file as test whose own claiming target is a
`java_library`, contradicting the rule that classification follows claims:

```python
java_library(name = "runner", srcs = ["Runner.java"])
java_test(name = "suite", test_class = "p.Runner", runtime_deps = [":runner"])
```

`Runner.java` is claimed by a `java_library` and stays production. The
reference is recorded as a test entry point and counted as
`test_class_entry_point`; unresolvable names count as
`test_class_unresolved`. A `java_test` with no `srcs` therefore classifies no
files, which is the honest reading of the build graph.

Indexing, per file:

1. Determine the owning unit from build metadata (below). Build metadata
   decides **only** `unit_id` and test/production classification.
2. Read the package solely from the top-level `package` declaration. No
   declaration means the default package. A malformed one makes the file
   unindexable and increments `skipped` once.
3. Index every named type declaration reachable as a **member** path: class,
   interface, enum, record, annotation type. Nested types are indexed under
   their full enclosing path, so `Outer`, `Outer.Inner`, and
   `Outer.Inner.Item` are three keys.
4. **Anonymous and local classes are not indexed, and not counted.** A local
   class declared inside a method body cannot be named under this scheme (its
   JVM name embeds the enclosing method) and cannot be imported by anything,
   so attempting to index it would inflate `skipped` on every file that uses
   one — manufacturing exactly the source-root-misdetection signal `skipped`
   exists to carry.
5. Deduplicate and sort type owner vectors by `(unit_id, module_id, context, file)` for
   deterministic output.

### Path cross-check

The path is used for diagnostics only. If a file's directory relative to its
detected source root does not match its declared package, increment `skipped`
once and **continue indexing under the declared package**. A mismatch must
never create the path-derived package, alias it, or redirect imports to it.

This is the direct symptom of a mis-detected source root — the primary way
this extractor could quietly under-report.

### Per-save cost

Java discovery builds a dedicated repository-wide `Project` beside the existing
`UnitMap`; per-file `UnitContext` cannot represent declaration ownership and
Bazel target visibility. Reparsing every Java file each time is not acceptable.
Discovery keeps a bounded process-local cache per repository root and a
best-effort `.phronesis/java-declarations.json` cache for separate hook
processes. Entries are fingerprinted on a **content hash**, matching the existing index
(`sync.rs:75`) rather than inventing a weaker scheme beside it. `(length,
mtime)` is not sound: a rebase, checkout, or file restore can produce
equal-length content under a preserved or same-granularity timestamp, and
`package a; class X {}` → `package b; class Y {}` is exactly that shape. The
cache would then keep resolving imports to a type that no longer exists —
violating the guarantee stated below. Every candidate cache reuse requires
hashing the current bytes; metadata alone cannot justify skipping the read.

- Cold: read and parse every Java file once. Unavoidable — package
  declarations and nested types cannot be obtained soundly from paths.
- Warm: read and hash the tracked Java files, reuse cached declarations for
  unchanged hashes, parse only changed files, drop deleted entries, and rebuild
  the maps. This saves parsing, not source reads.
- If metadata is unavailable, reread conservatively.

The disk cache is written atomically only when `.phronesis` already exists,
and does not create project configuration during discovery. Its format version,
engine version, and canonical repository root must match. Corrupt, oversized
(over 64 MiB), incompatible, and symlinked caches are misses; write failures
do not fail graph extraction. Changed and deleted sources replace/drop cached
entries, and build ownership/visibility is always recomputed. Increment the
cache format when declaration extraction or the grammar changes.
The file is disposable derived state and is covered by the generated
`.phronesis/*` ignore pattern.

The cache is an optimization only; reuse must never change identity or
resolution results. The earlier 6.5–10 ms per-save baseline does not describe
these Java corpus rebuilds. Measured release hooks with the disk cache take
about 2.2 seconds on Maven and 2.7 seconds on Bazel; see the corpus report
for sample sizes, methodology, and remaining full-rebuild cost.

## Import resolution

Resolution is **exact against the declaration index, or it does not happen.**
It never falls back from an unknown type or subpackage to an ancestor package.

"Exact" is a claim about *name* resolution, not about the JVM classpath. This
extractor reads declarations, not build closure, so cross-unit resolution is
constrained by declared dependencies (step 6) and is otherwise conservative:
it prefers emitting nothing to emitting a plausible guess.

For each file, `source_module` is its declared package within its unit. Then
per import:

1. **Plain type import** — `import com.example.order.Order;`
   Look up the entire dotted name in `types`. Do not remove segments.
2. **Nested-class import** — `import com.example.order.Outer.Inner;`
   Identical operation: nested types are indexed explicitly.
3. **Wildcard (type-import-on-demand)** — `import com.example.order.*;`
   Strip the terminal `*`, look up the remainder in `packages`; **if absent,
   fall through to `types`.**

   JLS §7.5.2 permits a wildcard on a *package or a type*, so
   `import com.example.dto.Response.*;` — importing the nested types of
   `Response` — is legal Java. A `packages`-only lookup finds nothing and
   emits no edge and no `skipped`, which is a silent drop of exactly the kind
   this design exists to prevent. Sealed hierarchies with nested permitted
   subtypes make this shape common in modern Java.
4. **Static member import** — `import static com.example.order.Order.of;`
   Remove exactly the final member identifier and look up the remaining
   canonical type name in `types`. JLS §7.5.3 defines this as
   `TypeName.Identifier`, not an arbitrary member path. Never remove further
   segments: that could resolve an invalid path through an unrelated ancestor
   type. See [JLS §7.5.3](https://docs.oracle.com/javase/specs/jls/se25/html/jls-7.html#jls-7.5.3).
5. **Static wildcard** — `import static com.example.order.Order.*;`
   Strip `.*`, look up the remainder in `types`.
6. **Module import** — `import module java.sql;`
   Recognised and resolved to nothing, counted as `module_import_ignored`.
   Java 25 added this form; it names a JPMS module, and modules are not in the
   declaration index (see JPMS, below). It must be *recognised* rather than
   left to fall through, or a legal import would inflate
   `skipped` — which is reserved for evidence this extractor failed to
   produce, not for a form it deliberately does not model.

**Supported source level: Java 25.** "Java" without a version boundary is not
a specification — `import module` did not exist two releases ago, and records,
sealed types, and pattern matching all changed what a declaration looks like.
The grammar targets 25 and a newer construct that fails to parse is counted,
not silently ignored.

Then, for any resolution that produced owners:

6. **Owner selection, constrained by build visibility.**
   a. First filter candidates by per-file build visibility, including candidates
      within the same Bazel unit. Then, if exactly one candidate is in the
      source file's own unit **and
      compilation context**, take it. Context is production or test: a
      production file sees production owners only; a test file sees test
      owners first, then production. `Owner` therefore carries the
      classification, not just the unit.

      Without this, `src/main/java/p/Environment.java` and
      `src/test/java/p/Environment.java` are two same-unit owners, step (a)
      finds no unique candidate, and every import of `p.Environment` is
      dropped as ambiguous — though Maven's test compilation resolves it
      unambiguously, with the test declaration shadowing the main one.
   b. Otherwise, restrict candidates to units **visible** to the source unit.
      If exactly one survives, take it.
   c. Otherwise emit nothing and increment `skipped` once.

   Step (b) is not optional politeness. Exact presence in the index is **not**
   classpath visibility: a type uniquely declared in an unrelated module that
   the source cannot compile against would otherwise produce an edge Java
   itself cannot resolve, manufacturing a package cycle between build units
   that do not depend on each other. That corrupts the one rule this
   extractor ships.

   Where a backend exposes no usable dependency list, cross-unit candidates
   are skipped rather than guessed.

7. **Self-edge suppression.** If `target_module == source_module`, emit
   nothing. This is mandatory: a redundant same-package import is legal Java,
   and `derive::in_cycle` treats `imports(a, a)` as a cycle — a lone SCC node
   counts when it is its own successor (`derive.rs:84`, asserted at
   `derive.rs:336`) — so an unsuppressed self-edge is a phantom cycle report.
8. Otherwise emit `imports(source_module, target_module)`. Deduplicate per
   file.

**Same-package references need no algorithm.** A reference to `Invoice` from
another file in `com.example.billing` resolves to the same module node, so it
would be suppressed by step 7 anyway. It is a non-edge by construction — which
is the whole point of package granularity.

### Build visibility

**Visibility is defined per backend.** Direct `<dependencies>` alone is not
the Maven compile classpath, so state the approximation rather than imply
exactness:

**Visibility models the compilation context's classpath.** Production and
test compilation have different classpaths. Maven runtime dependencies are
excluded from production compilation, but may be present for test compilation.
Bazel `runtime_deps` never grants direct source-level import visibility.
Same-package labels accept both `:target` and bare `target`, including
dependencies, exports, aliases, and local filegroup references. Omitting the
colon does not make the dependency external or unresolvable.
Dependency aliases follow literal `actual` labels and the union of string
branches in `actual = select(...)`, including values assigned to variables.
These unions increment `select_branch_unioned`; they do not claim knowledge
of the active configuration. Alias cycles increment `unresolved_label`, while
shared targets reached through independent branches are not cycles. A
`select` mixing string and list branches is unsupported syntax.

- **Maven.** Build a reactor graph from `<dependencies>` entries whose
  `groupId:artifactId` matches another discovered unit. Propagate an effective
  scope along each dependency path using Maven's published table:

  | Effective scope so far | Next `compile` | Next `runtime` | Next `provided` or `test` |
  |---|---|---|---|
  | `compile` | `compile` | `runtime` | omitted |
  | `provided` | `provided` | `provided` | omitted |
  | `runtime` | `runtime` | `runtime` | omitted |
  | `test` | `test` | `test` | omitted |

  Production compilation admits effective `compile` and `provided`; test
  compilation also admits effective `runtime` and `test`. Direct dependencies
  start with their declared scope. Track visited `(unit, scope)` pairs so
  different paths are preserved and cycles terminate. This replaces revision
  6's overly narrow `compile`-only closure. See the
  [Maven dependency scope table](https://maven.apache.org/guides/introduction/introduction-to-dependency-mechanism.html#dependency-scope).

  `<optional>`, `<exclusions>`, and **`<dependencyManagement>`** — including
  BOM imports — remain explicit approximations, counted as
  `dependency_modifier_ignored`. A classifier, non-jar type, or `systemPath`
  artifact cannot be mapped to a unit's main declarations just by matching
  coordinates: omit that dependency and count `dependency_artifact_unsupported`.
  The scope evaluator supports the classic compile/provided/runtime/test
  table above. Maven 4 `compile-only`, `test-only`, and `test-runtime` scopes
  are currently unsupported and omitted with the same diagnostic; the Maven
  corpus report accounts for the affected fixture imports.
  Dependencies on artifacts outside the reactor have no indexed source
  candidates and create no project edges. External units expose their
  production declarations only; test-jar source ownership is not modelled.

- **Bazel.** `java_library` defaults to `strict_deps = True`, so only **direct
  `deps`, extended through `exports`**, are on the compile classpath. A
  transitive closure would admit most of the build graph.

  **Visibility here is per *file*, not per unit.** This is the one place the
  package-level unit is not enough: `deps` are declared per target, and two
  targets in one `BUILD` file routinely have different ones.

  ```python
  java_library(name = "a", srcs = ["A.java"], deps = ["//x"])
  java_library(name = "b", srcs = ["B.java"], deps = ["//y"])
  ```

  Unioning these would let `A.java` resolve a type from `//y` that Bazel
  rejects; intersecting them would drop `A.java`'s legitimate `//x` imports.
  Neither is acceptable, so the evaluator retains, for every claimed file, the
  union of `deps`+`exports` of **the targets that claim that file** — normally
  one. Unit identity stays package-level for module naming; only visibility is
  target-derived. Unresolvable labels are counted as `unresolved_label`;
  `visibility` attributes are not evaluated.

**JPMS is not modelled, and that is a visibility gap, not just a parsing
one.** A `module-info.java` that omits `requires` makes types unreadable even
when they are on the classpath, so a modular project can have edges this
design invents. Any unit containing a `module-info.java` is counted as
`jpms_unit_unmodelled`, and a corpus with a non-zero count cannot be used to
argue precision without saying so.

**This approximates the real classpath in both directions**, and it is worth
naming which is which rather than claiming a single safe side:

- **Under** — requiring a declared dependency path drops any edge the build
  resolves by some route this spec does not model. Dropped edges land in
  `skipped` and are reconciled at the corpus gate.
- **Over** — ignoring `<exclusions>`, `<optional>`, Bazel `visibility`, and
  JPMS `requires` admits a unit the build would reject. This is the direction
  that can invent a cycle.

Each over-approximating case has a counter — `dependency_modifier_ignored`,
`jpms_unit_unmodelled` — so a corpus where they are common is a signal to
model them before shipping rather than to trust the result. Given the one rule
shipped is cycle detection, an invented edge is the more damaging error, and
it is the one being measured.


### The third-party ancestor case

This is what revision 1 got wrong. Given only `package com.acme` in the
project:

```java
import com.acme.vendor.Widget;   // supplied by a dependency
```

No edge. `com.acme.vendor.Widget` is not in `types`, and the existence of
project package `com.acme` is irrelevant. Revision 1's longest-prefix rule
emitted a false `imports(…, com::acme)` here.

### `skipped` accounting

**`skipped` is independent of the named discovery counters and never
aggregates them.** Named counters (`unresolved_property`,
`select_branch_unioned`, `files_unclaimed`, `multi_release_root_ignored`, …)
are reported separately. Mixing them would make corpus reconciliation
double-count, and they measure different things: `skipped` counts *evidence
this extractor could not produce*, while the named counters count *decisions
it deliberately made*.

`skipped` increments once for each of: an unparseable package declaration; a
declared package that disagrees with its path; a named member type declaration
that cannot be named; a syntactically unsupported import; and a lookup left
ambiguous after owner selection. It is counted **per occurrence**; named
counters are counted **per distinct object** (root, label, profile, property).

Do **not** increment for: an exact lookup absent from both indexes (that is
third-party, and normal); `java.*` / `javax.*` / `jakarta.*`; a suppressed
self-edge; or an unresolved unqualified name, which may be a type parameter,
an inherited member type, or a `java.lang` type this syntax-only index cannot
see.

**Unused-import suppression is not in v1.** Revision 1 proposed requiring the
imported simple name to appear in the body. That is not Java name resolution —
an identifier can be a type parameter, variable, method, or annotation — and
it is wrong for static imports, where the used name is the member, not the
type. An explicit import is itself a compile-time dependency declaration;
suppressing it on textual grounds trades a rare false positive for unsound
false negatives.

## Discovery: Maven

Every `pom.xml` is examined. Discovery answers only "which unit" and "is this
a test."

- **Unit id** is `groupId:artifactId`, with `groupId` inherited from the
  parent chain when absent.
  A literal `groupId` in the child's own `<parent>` element remains usable
  when the parent POM is external: it is declared coordinate evidence in the
  local file. This does not import the unavailable parent's build settings,
  dependencies, or properties.
- **Source roots** default to `src/main/java` (production) and
  `src/test/java` (test); `<sourceDirectory>` / `<testSourceDirectory>`
  override them.
- **Property interpolation** supports `${project.basedir}`,
  `${project.groupId}`, `${project.artifactId}`. A path still containing an
  unresolved `${…}` is discarded in favour of the defaults, counted as
  `unresolved_property`.
- **Parent POMs** are followed via `<relativePath>` (default `../pom.xml`)
  when the target is inside the repository; child config overrides parent.
  Parents resolvable only from an external repository are not fetched, counted
  as `external_parent_skipped`.
- **`<packaging>pom</packaging>`** aggregators define no unit. Their
  `<modules>` need no special handling — each child has its own `pom.xml` and
  the walk finds it.
- **`build-helper-maven-plugin`** `add-source` / `add-test-source` executions
  are merged by execution ID before roots are materialized. Child execution
  configuration overrides parent configuration; plugin-level configuration
  supplies execution defaults. `combine.self="override"` and
  `combine.children="append"` control configuration merging. Plugin and
  execution `<inherited>false</inherited>` suppress inheritance (an execution
  can explicitly opt back in); `<phase>none</phase>` disables an execution.
  See the [Maven POM reference](https://maven.apache.org/pom.html) and the
  [Maven model merger](https://github.com/apache/maven/blob/ea4a417bd2482448b8a5b2cf83f2738eee99d47f/impl/maven-impl/src/main/java/org/apache/maven/impl/model/MavenModelMerger.java).
  Any other plugin that appears to modify source roots is
  counted as `unsupported_plugin_source_modifier` and ignored.
- **Profiles** are merged only when `<activeByDefault>true</activeByDefault>`.
  Profiles activated by file existence, environment, system property, or `-P`
  are counted as `inactive_profile_skipped`.
- **Multi-release roots** (`src/main/java9`, `src/main/java11`) are **not
  indexed in v1**, counted once per distinct root as
  `multi_release_root_ignored`. Indexing them alongside the base root would
  declare the same type twice in one unit and turn a layout convention into
  `skipped` noise; runtime selection is version-sensitive and cannot be
  modelled by a flat index.

**Root resolution is ordered**, because a versioned root can arrive by any of
the routes above and the rules would otherwise contradict each other:

1. Start from defaults.
2. Apply `<sourceDirectory>` / `<testSourceDirectory>` overrides.
3. Merge default-active profiles.
4. Merge `build-helper` added roots.
5. Normalise all roots to repo-relative paths, **then** remove any matching
   `.../java[0-9]+$`, counting each.
6. Assign file ownership from what remains.

Stripping versioned roots last means a root added by `build-helper` or a
profile is treated identically to one that was there all along, so
implementation order cannot change behaviour or counters.

## Discovery: Bazel

**One unit per Bazel package** (the `BUILD` file's directory), **not per
target.** Per-target units would split a Java package across nodes —
`java://foo:lib::com::example::billing` and
`java://foo:tests::com::example::billing` — because a test class declares the
same `package` as the class it covers. Test→production imports would then read
as cross-package edges and manufacture phantom cycles. Maven avoids this
structurally, since `src/main/java` and `src/test/java` share one `pom.xml`;
Bazel must be told.

`BUILD` files are evaluated by a **restricted expression evaluator**, not
matched by pattern. Revision 1 specified "literal lists and `glob()`", which
cannot read ordinary production Bazel:

```python
COMMON = glob(["src/**/*.java"], exclude = ["**/testdata/**"])
java_library(name = "lib", srcs = COMMON + select({":ent": ["E.java"], "//conditions:default": []}))
```

### Grammar

```ebnf
File       ::= Statement*
Statement  ::= Assignment | RuleCall
Assignment ::= IDENT "=" Expr
RuleCall   ::= IDENT "(" [Arg ("," Arg)* [","]] ")"
Arg        ::= KwArg | Expr
KwArg      ::= IDENT "=" Expr
Expr       ::= Primary ("+" Primary)*
Primary    ::= String | List | Glob | Select | IdentRef
IdentRef   ::= IDENT
List       ::= "[" [Expr ("," Expr)* [","]] "]"
Dict       ::= "{" [Entry ("," Entry)* [","]] "}"
Entry      ::= String ":" Expr
Glob       ::= "glob(" (List | "include" "=" List) ["," "exclude" "=" List] ")"
Select     ::= "select(" Dict ")"
String     ::= '"' char* '"' | "'" char* "'"
IDENT      ::= letter (letter | digit | "_")*
```

Concatenation is **iterative, not left-recursive**, so a recursive-descent
parser handles it directly and `+` is unambiguously left-associative.

**Labels are quoted strings**, not a syntactic form — `":helpers"` and
`"//other:files"` are `String` values recognised semantically where a label is
meaningful (`srcs`, `deps`, `select` keys). Starlark has no bare label syntax.

**Positional arguments parse but are ignored.** A macro written
`my_java_lib("name", ["A.java"])` will not fail the file, but its sources
cannot be identified as `srcs` without the attribute name, so it claims
nothing. This is a stated limit, and its files surface as `unclaimed`.

Attributes unrelated to ownership and visibility (such as integer timeouts)
do not invalidate a target's known `srcs`. Relevant attributes use the restricted
expression subset below; unsupported relevant expressions skip the statement.

Anything outside this grammar in an ownership-relevant expression skips that statement and increments
`unsupported_syntax_skipped`. No functions, loops, comprehensions, string
formatting, `depset`, or `struct`.

### Evaluation

Evaluation has a per-BUILD cumulative allocation budget of approximately 8 MiB
for string contents and list entries. Identifier copies, string concatenation,
select unions, and glob results consume that budget. Exhaustion records
`evaluation_budget_exceeded` and stops the rest of the file; already evaluated
targets remain available. This bounds exponential value expansion, not total
process RSS or parser input size.

**Pass 1 — bind.** Evaluate top-level assignments in order into a local scope,
then evaluate each rule call's arguments against it, recording
`(rule_kind, attrs)`.

**An identifier not yet in scope evaluates to the empty list**, and that
attribute is counted as `unbound_identifier`. A forward reference or a name
introduced by `load()` is *inside* the grammar but semantically unresolved, so
"unsupported syntax" does not cover it. Without a stated rule an implementer
could equally skip the statement, fail the file, or drop the concatenation —
three different edge sets from one input. Empty-list is chosen because it
degrades to "claims fewer files", which surfaces as `unclaimed` rather than as
a silently wrong claim.

**Pass 2 — claim and classify.**

1. `glob(include, exclude)` is evaluated against **all `.java` files in the
   Bazel package**, never against a shrinking pool of unclaimed files.
   Globbing a residual set would make results depend on target order within
   the `BUILD` file. `*` spans one path segment, `**` spans many; exclude wins
   over include.

   **A Bazel package stops at any subdirectory containing `BUILD` or
   `BUILD.bazel`.** A recursive glob must not descend into a nested package.
   Without this, `foo/BUILD` globbing `src/**/*.java` would claim
   `foo/sub/B.java` that `foo/sub/BUILD` also claims, making unit assignment
   depend on traversal order. The directory walk and the glob evaluator share
   one boundary function and recognise both filenames.
2. `select({...})` **unions all branches.** A file in any branch is a project
   source. This over-approximates: a file built only under a non-default
   config is still claimed. Counted as `select_branch_unioned`.
3. **Any rule call with a `srcs` attribute claims those files**, regardless of
   rule name, except `filegroup`, which is a source container, not a compilation
   target. Files reached through a filegroup inherit the consuming target's
   classification. An unrecognised macro with `srcs` still claims its sources.
4. Local `:label` references resolve to a `filegroup` in the same file, whose
   `srcs` are claimed transitively. Cross-package `//pkg:target` labels are
   not resolved, counted as `unresolved_label`.
5. **Test classification.** Because globbing from the full package lets two
   targets claim one file, classification needs a precedence rule:
   **`file_type` is single-valued, and production wins.** A file claimed by
   both a `java_library` and a `java_test` is `production`, counted as
   `dual_claimed_file`.

   Production is the safer direction, and the reverse of what an earlier draft
   claimed. Rules exempt test files, so labelling production code as test
   *suppresses* findings about it — a silent false negative, the failure mode
   this whole subsystem is organised against. Labelling test code as
   production merely produces noise a human can see and dismiss. It is also
   more accurate: a file compiled into a `java_library` is production code by
   the build's own account, whatever else also consumes it.

   `java_test` with `srcs` marks those files as test.
   `java_test` with `test_class` and no `srcs` resolves the class name through
   the **declaration index** — the same index resolution needs — to find its
   file for entry-point diagnostics only; it does not change classification.
   A rule whose name ends in `_test` classifies its claimed files as
   test, which covers the common macro case.
6. Files present but claimed by nothing belong to the Bazel package's unit and
   are emitted as `file_type(file, "unclaimed")`, counted as
   `files_unclaimed`. They are not silently dropped and not silently called
   production.

`unclaimed` extends the `file_type` vocabulary (`production`, `test`, `build`,
`example`). It is inert for v1's only rule, which does not join `file_type`,
and it keeps a discovery gap visible in the graph itself rather than only in a
counter.

`target/`, `build/`, and `bazel-*` output are left to `.gitignore`. Java needs
no `node_modules` analogue: dependencies are jars outside the tree.

## Mixed ownership and path diagnostics

Prefer the owner whose build manifest directory is closest to the source.
At equal manifest depth, a matching Maven source root takes precedence over
Bazel ownership, and `mixed_build_owner_selected` identifies every overlap.
Within one Maven unit, production wins overlapping production/test roots;
within the chosen context, use the longest matching root. Sources outside any
claimed Maven root fall back to a containing Bazel package or `java:project`.
Explicitly configured versioned roots, plus conventional
`src/main/java[0-9]+` and `src/test/java[0-9]+` roots, are excluded before
fallback. A package segment named `java11` beneath a normal source root is
not an exclusion.

For Bazel path diagnostics, first recognize ancestor source roots named
`src/main/java`, `src/test/java`, `javatests`, or `java`. BUILD files may live
inside a declared Java package beneath that root. Then consider those roots
and `src` relative to the Bazel package; otherwise use its package directory. This is only a diagnostic
heuristic. It never creates a package identity. Corpus path mismatches require
reconciliation because arbitrary Bazel source layouts remain legal.

## Invalidation

`sync.rs` currently tracks source extensions only, and its compaction is
per-edited-file. Java breaks that assumption: **build metadata reclassifies
files that were never touched.**

Changing a `<sourceDirectory>`, an `artifactId`, a parent POM, a `glob`
pattern, or a `java_test`'s `srcs` can rename or reclassify hundreds of files
in one edit. Per-file compaction would leave every one of their persisted
edges stale, and the graph would report a layout that no longer exists.

Therefore:

- `.java` joins `TRACKED_EXTENSIONS`, `tracked_files`, `is_tracked`,
  `lang_of_path`, and the `extract_one` dispatch.
- `pom.xml`, `BUILD`, and `BUILD.bazel` are tracked for **freshness**, and a
  change to any of them routes to `rebuild(root)` — the same escape hatch
  `sync.rs:267` already uses when the graph format changes — rather than to
  `on_save`. They produce no edges themselves.

  A per-file path is not merely slower here, it is **wrong**.
  `store::compact(existing, file_path, edges)` replaces edges by their
  provenance file (`sync.rs:288`), so it can only touch files it is handed.
  A manifest edit that stops a file being discovered, or moves it to another
  unit, leaves the old edges in place under a provenance nothing will revisit
  — the identical reasoning the format-change comment already gives for
  rebuilding rather than patching.
- The declaration-index cache is keyed on repository root and invalidated
  wholesale when any tracked manifest changes, since unit ids and
  classifications are inputs to every Java identity.

Java declarations are also repository-wide resolution inputs. Adding,
removing, moving, or changing a Java file can alter the target or ambiguity of
imports in untouched files. The initial implementation must rebuild on any
Java source save or deletion, as well as on manifest changes. Optimizing this
later requires a reverse dependency index that includes unresolved lookups;
refreshing only currently resolved importers misses newly resolvable imports.
A rebuild invoked by `on_save(root, path, content)` must use the supplied
content for that path, rather than accidentally indexing old disk contents.

Integration tests must cover a manifest-only edit — changing a source root
with no `.java` file touched — and assert the graph reclassifies. They must
also cover a declaration rename, added ambiguity, removal of ambiguity, and
file deletion, checking imports from unchanged source files against a clean
rebuild. A source root removed from a Maven unit must leave no edges under
its old identity (unclaimed sources follow the documented fallback policy).

## Relations

`file_type`, `declares_module`, `defines_fn`, `imports`, `calls_api`,
`tested_by` — the existing closed set, no additions.

- **`declares_module(file, module)`** is many-to-one for Java: every file in a
  package emits the same module. The model supports this: edges are stored as
  `(predicate, args)` with provenance keyed on the source file, so compaction
  stays per-file, and the cycle rule joins through `edited_file` to report the
  file in front of the user.

  **Provenance is storage-only.** `Edge::fact_id` is built from predicate and
  args alone (`model.rs:55`), so `imports(p, q)` emitted from two different
  Java files hydrates to **one** fact. That is harmless for `in_cycle`, which
  derives before hydration, but no rule can attribute an `imports` fact to a
  particular file — the attribution available to rules comes from
  `declares_module`, not from the import edge.
- **`tested_by(callee, test_fn)`** — only calls inside methods annotated
  `@Test`, `@ParameterizedTest`, `@RepeatedTest`, or `@TestFactory`, in a file
  the build system classified as test. A helper's calls are not evidence.
  Explicit `this` calls never resolve through static imports. Methods
  declared on the current type precede imported methods; explicit static
  imports precede wildcard static imports. Receiver types are resolved before
  method names, with lexical member types preceding explicit type imports,
  which precede wildcard imports. Ambiguous visible receiver types or
  applicable overloaded candidates suppress coverage rather than being
  discarded to make another candidate appear unique. Direct construction
  (`new Service<T>().run()` or `(new p.Service<T>()).run()`) supplies an
  instance receiver type. Anonymous subclasses, factory results, and ordinary
  variable receivers do not supply that evidence. Unresolved ancestry and
  inherited `Object` method names prevent unqualified calls from falling
  through to same-named static imports.
- **`file_type`** carries `production`, `test`, or — Bazel only —
  `unclaimed`. A Maven file under a production source root is `production`,
  one under a test root is `test`; Maven never yields `unclaimed`, because a
  `pom.xml` claims its roots wholesale. The existing `build` and `example`
  values are Rust-specific and are never emitted for Java.
- **`package-info.java`** emits `declares_module`, `file_type`, and any
  `imports` carried by its package annotations, but no `defines_fn`. It
  participates in the path cross-check like any other file.
- **`calls_api`** is emitted for `Optional.get()`-shaped calls (zero-argument
  `.get()`, `.orElseThrow()`, and the `OptionalInt`/`Long`/`Double`
  accessors), but **no v1 rule consumes it** — see below.

## Rules shipped

| id | severity | fires on |
|---|---|---|
| `warn-import-cycle` (existing `structural` pack) | `warn` | edited Java file whose declared package is in an import cycle |

**Java uses the existing language-neutral cycle rule.** The current pack
already applies it to every `declares_module`/`in_cycle` pair. Adding the earlier
draft's `warn-java-import-cycle` would duplicate every Java cycle warning.
The real-binary tests must prove the existing rule reaches Java findings.
No Java risky-call rule is introduced in v1.

The original revision described repository-global short-name coverage. The
current implementation instead canonicalizes function edges in
`derive::canonicalize_function_edges`, and `sync/rebuild.rs` rejects
`tested_by` targets absent from `defines_fn`. Java must respect that invariant:
emit coverage only for a uniquely resolved canonical function identity, and
omit ambiguous or unresolved calls. Do not rely on a shared method leaf name
to infer receiver identity. Overloaded Java methods still need an explicit
identity and resolution policy before a risky-call rule can be trusted.

Enabling it later requires verifying the canonicalization path and Java
extraction together — a cross-language decision that belongs in its own ADR:

1. `tested_by` must carry a resolvable qualified identity, not a bare name
   (already enforced at persistence).
2. Coverage derivation must preserve exact identities through canonicalization.
3. A call that cannot be resolved to exactly one defined function must emit no
   `tested_by` edge — ambiguity must *reduce* claimed coverage, not spread it.
4. Java identity must carry an overload discriminator, minimally arity
   (`…::Store::get/1`), preferably erased parameter types.
5. `calls_api` and `defines_fn` must share that overload-aware identity so the
   join stays exact.

Until then, Java's shipped feature is advisory package import-cycle detection
under the build-visibility approximations described above.

## Out of scope

Java's real structural blind spots, recorded as known limits rather than
corpus surprises.

- **Reflection and dependency injection.** Spring `@Autowired` by type,
  `Class.forName`, `ServiceLoader`. Java leans on these harder than Rust or
  Python, and no import-based extractor sees them. This is the largest
  category of genuinely missing edges and is not addressable at this tier.
- **Generated sources.** Lombok, MapStruct, annotation processors, Bazel
  `genrule` output. The code does not exist until build time.
- **Fully-qualified inline usage** — `com.example.order.Order o = new
  com.example.order.Order();` uses no import and produces no edge.
- **Cross-package Bazel labels**, external parent POMs, non-default profiles,
  multi-release roots — each counted, per Discovery.
- **Class-level coupling edges.** A rule like "this class has too many
  dependencies" needs class→class edges. The relation set is closed on purpose
  (the TypeScript spec rejected an intermediate relation for the same reason),
  so adding one is a deliberate event.
- **JPMS `module-info.java`.** Skipped entirely: not indexed, no edges,
  counted once per file as `module_info_skipped`. It is a `.java` file with no
  `package` declaration, so indexing it would file it under the default
  package, and its `requires` directives are not imports. Silently defaulting
  it would put a fictitious member in the default-package node.
- **Gradle**, Kotlin/Scala, and promotion to `block`.

## Testing

Unit tests per concern, following the Python and TypeScript extractors:
identity from the `package` declaration; a wildcard naming a *type* rather
than a package; `import module`; production and test source sets declaring the
same FQN in one unit; the declaration cache invalidating on equal-length,
same-mtime content; two Bazel targets in one file with different `deps`
resolving independently; a `java_test` with `test_class` and no `srcs`
reclassifying nothing; nested-type indexing three levels
deep; the path cross-check incrementing `skipped` without creating the
path-derived package; each of the five import forms; the third-party-ancestor
case emitting **no** edge; self-edge suppression; owner selection across a
split package including the ambiguous case; Maven property interpolation,
parent inheritance, `packaging=pom`, `build-helper`, and default-active
profiles; the Bazel evaluator over `COMMON + select(...)`, `glob` exclude,
`filegroup` indirection, an unknown macro with `srcs`, `java_test` with
`test_class` and no `srcs`, and a `java_library` plus `java_test` in one
`BUILD` file yielding **one** unit with two classifications; a nested `BUILD`
halting a parent's recursive glob; a file claimed by both a library and a test
resolving to `production`; a wildcard import into a three-file package resolving to
**one** owner; and a cross-unit type that is unique but not a declared
dependency emitting **no** edge.

Integration tests through the real binary, extending
`tests/graph_structural_rules.rs`: a Maven project and a Bazel project each
producing a graph, a package cycle detected, the rule firing, and a mixed
Rust/Python/TypeScript/Java repository proving four languages coexist.

### Corpus validation is a merge gate

**Validation against real projects is required before merge, not optional.**
phronesis contains no Java, so there is no in-repo corpus. Synthetic fixtures
will pass while source-root detection quietly mis-fires on real layouts — the
failure mode this design is organised around.

Two corpora, one per discovery backend, since they share no code path:

- **Maven:** a multi-module, pure-Maven project — `apache/maven` itself is the
  leading candidate.
- **Bazel:** a Java tree built with Bazel — `bazelbuild/bazel` is the leading
  candidate, and its heavy macro use exercises the documented blind spots
  rather than hiding them.

Final selection is confirmed at implementation time by cloning each and
verifying its build files, not assumed from this document.

Each run reports: units, modules, `imports` edges, every counter named in
Discovery, `skipped`, and detected cycles. `skipped` and `files_unclaimed` are
reconciled against a human reading of the layout, and a sample of detected
cycles is confirmed by hand. A `skipped` count that is not near-zero means
source-root detection is wrong, and the extractor does not merge until it is
explained.

## Risks

1. **Source-root misdetection under Bazel.** Highest likelihood. Mitigated by
   the path cross-check, which is aimed directly at this symptom, and by the
   corpus gate.
2. **Declaration-index cost.** Cold discovery reads every Java file. Mitigated
   by the content-hash declaration cache; if a large corpus still bites, cache
   to disk beside `graph.jsonl`. Measure on both corpora.
3. **Bazel evaluator coverage.** The grammar is a subset by design. Every gap
   is counted, and a large `files_unclaimed` or `unsupported_syntax_skipped`
   on the corpus is a signal to extend the grammar before shipping, not to
   ship quietly.
4. **Package-granular cycles read as over-reporting.** Coarser by design.
   Mitigated by rule wording that claims package interdependence, and by
   hand-confirming a sample of corpus cycles.
5. **Split packages produce ambiguity rather than edges.** Owner selection
   prefers the local unit and otherwise requires uniqueness. A corpus with
   many split packages would show up as `skipped`; if that is common in
   practice, revisit whether the unit belongs in Java module identity at all,
   since the JVM classpath merges packages across artifacts.

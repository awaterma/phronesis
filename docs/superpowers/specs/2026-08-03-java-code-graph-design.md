# SPEC: Java code-graph extractor

**Status:** design, approved 2026-08-03
**Target release:** 0.26.0 (MINOR — new extractor, new pack rules)
**Parent spec:** `docs/specs/SPEC-triple-store-rete.md` (rev 6)
**Affects:** `crates/phronesis-mcp/src/graph/{unit,java,sync,mod}.rs`,
`crates/phronesis-mcp/src/init.rs` (pack rules), `docs/catalogue.html`
**Precedent:** `docs/superpowers/specs/2026-07-31-typescript-code-graph-design.md`

## Summary

A fourth extractor for the structural code graph, covering Java, with unit
discovery from **both Maven and Bazel**.

Java inverts TypeScript's difficulty. TypeScript's hard part was resolution:
imports name filesystem *paths* that must be probed before an edge can be
drawn. Java imports name fully-qualified types, and every Java file declares
its own package, so identity is read directly from the source and resolution
needs no disk access at all.

The hard part here is **granularity**. Java is the first language in the graph
whose namespace is not its file. A package spans many files, and two classes
in the same package refer to each other with no import statement. That single
fact determines the whole design.

## Why granularity is the risk

A missing edge is invisible. It does not error and it does not look different
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

There is no import to scan, so there is no edge to draw. And this is not an
edge case: Java developers deliberately co-locate collaborating classes, so
class-level extraction would capture the sparse cross-package edges while
systematically dropping the dense intra-package ones. The result is a graph
that is most wrong exactly where coupling is highest.

**Consequence for the design:** the module is the package, not the class. That
does not mitigate the hole — it removes it. Two classes in one package are one
node, so there is nothing left to miss.

## Identity

`java:<unit>::<package path>`, segments joined with `::`, no target suffix.

```
src/main/java/com/example/billing/Charge.java  →  java:myapp::com::example::billing
src/main/java/com/example/order/Order.java     →  java:myapp::com::example::order
```

**The package comes from the file's own `package` declaration, not from its
path.** Java states its namespace explicitly; nothing needs to be inferred.
The path is used only as a cross-check (see `skipped`, below).

Functions keep full precision, because `defines_fn` names the function rather
than the module:

```
java:myapp::com::example::billing::Charge::charge
```

Package, class, and method are all present. Choosing package-level *modules*
costs class-level *edges*, not class-level *identity*. This matches the
existing TypeScript shape (`typescript:myapp::billing::Ledger::charge`).

A `.java` file no manifest claims falls back to `java:project`, matching
Python and TypeScript. Segments join with `::` in every language; see
`.phronesis/wiki/decisions/2026-07-27-graph-identity-separator.md`.

### What package granularity claims

`in_cycle` runs only over `imports`, so package-level modules make the cycle
rule a package-level claim. It is coarser than a class-level cycle and can
fire where no class-level cycle exists:

`billing.Charge → order.Order`, and separately `order.Shipment →
billing.Invoice`, is a `billing` ⇄ `order` cycle. Those four classes form a
chain that never closes.

This is intended, and it is recorded here so the rule's wording stays honest.
The rule claims **"these two packages are mutually dependent — neither can be
extracted without the other,"** not "there is a circular reference between
classes." The first is what jdepend and ArchUnit report and what Java
architects act on. The second is a narrower, different finding.

The same reasoning that gave TypeScript's `!` rule a deliberately weaker claim
than Rust's `.unwrap()`: state what the evidence supports, not what sounds
strongest.

## Discovery

Because identity comes from the `package` declaration, the build system never
touches naming. It answers exactly two questions:

1. **What is the unit name?**
2. **Is this file a test?**

That is the entire contract, and it is why supporting two build systems costs
roughly one extra function rather than doubling the design. Each backend is
independently testable against that two-question interface.

**Maven** — every `pom.xml` defines a unit.

- Unit name from `<artifactId>`.
- Source roots are conventional: `src/main/java` (production) and
  `src/test/java` (test). `<build><sourceDirectory>` and
  `<testSourceDirectory>` override them and are honoured — `pom.xml` is XML,
  so these are data, not a program.
- `<modules>` needs no special handling: each child module has its own
  `pom.xml`, so the walk finds it, and the existing innermost-unit rule
  resolves nesting.

**Bazel** — every `BUILD` / `BUILD.bazel` file defines units.

- **One unit per Bazel package (the `BUILD` file's directory), not per
  target**, named by its package label (`//path/to`). A directory's
  `java_library` and `java_test` targets are one unit.

  Per-target units would split a Java package across two nodes —
  `java://foo:lib::com::example::billing` and
  `java://foo:tests::com::example::billing` — because a test class declares
  the same `package` as the class it covers. Test→production imports would
  then read as cross-package edges, inventing dependencies and, through them,
  phantom cycles. Maven avoids this structurally, since `src/main/java` and
  `src/test/java` share one `pom.xml`; Bazel has to be told.
- `srcs` is read from literal lists and `glob()` patterns.
- Test classification is **structural**: membership in a `java_test` target's
  `srcs`. This is the only thing target kind is used for. No naming convention
  required.
- Starlark is a program, not data. Only the declarative subset above is
  parsed. Macro-generated and rule-generated targets are invisible; files they
  claim fall back to `java:project` and are counted, not silently absorbed.

`target/`, `build/`, and `bazel-*` output directories are left to
`.gitignore`, which covers them in practice — the same call TypeScript made
for `dist/`. Java has no `node_modules` analogue: dependencies are jars in a
cache outside the tree, so no unconditional filter is needed.

**Java is the first language where the build system declares what a test is.**
Rust infers from `#[test]`, Python from `test_*`, TypeScript from a title
string inside a callback. Maven's source-root split and Bazel's `java_test`
rule kind are declarations. `@Test` / `@ParameterizedTest` annotations refine
*within* a test file; the build system decides *which* files those are.

## Resolution

Discovery collects the set of packages the project declares. This is derived
from **paths relative to source roots, with zero file reads** — a path under a
known source root yields its package name directly.

Resolution is then one rule:

> An import resolves to the **longest prefix of its dotted path that is a
> declared project package**. No match means third-party — no edge.

| Import | What it names | Resolves to |
|---|---|---|
| `com.example.order.Order` | one class | `com.example.order` |
| `com.example.order.*` | every class in the package | `com.example.order` |
| `static com.example.order.Order.of` | one static member | `com.example.order` |
| `com.example.order.Outer.Inner` | a nested class | `com.example.order` |
| `java.util.List` | JDK type | — no edge |

Four different things, deliberately projected onto one fact: *this package
depends on that package.* That is the fact `in_cycle` consumes, and it is true
of all four.

The projection is not a concession — it is what makes Java's awkward import
forms stop being special cases. Class-level extraction would need to strip one
segment for a class, two for a static import, and an unknown number for nested
classes (`Outer.Inner` is indistinguishable from `subpkg.Klass` without a
symbol table, since capitalisation is convention rather than rule). A wildcard
would be **unresolvable outright**: naming which classes `com.example.order.*`
pulls in requires indexing the package and scanning the body for identifier
usage — a symbol table, not an import scan.

Longest-prefix never has to classify a segment, so all five rows above fall
out of one lookup with no disk access on the per-save path.

**An import is only counted as an edge if the imported simple name also
appears in the file body.** An import left behind after a refactor otherwise
yields an edge for a dependency that no longer exists. This is cheap here and
removes a known false-positive class; it does not apply to wildcards, which
are exempted.

### What `skipped` counts

A file whose declared `package` disagrees with its path relative to the
detected source root.

This is the Java analogue of TypeScript's unresolved-relative-import counter,
and it is aimed at the right thing: a mismatch is the direct symptom of a
**mis-detected source root**, which is the primary way this extractor could
quietly under-report — and the primary risk in the Bazel backend, where source
roots are not conventional.

## Relations

`file_type`, `declares_module`, `defines_fn`, `imports`, `calls_api`,
`tested_by` — the existing closed set, no additions.

- **`declares_module(file, module)`** is many-to-one for Java: every file in a
  package emits the same module. The model supports this — edges are
  `(predicate, args)` with provenance keyed on the source file, and the cycle
  rule joins through `edited_file`, so it still reports the file in front of
  the user rather than an arbitrary member of the package.
- **`file_type`** is `test` or `production`, decided by the build system as
  described in Discovery.
- **`tested_by(callee, test_fn)`** — only calls inside methods annotated
  `@Test` / `@ParameterizedTest` in a file the build system classified as a
  test count as evidence. This mirrors Python's rule that only `test_*`
  functions are evidence: a helper's calls are not proof anything was
  verified.
- **`calls_api(fn, api)`** — watchlist below.

### `calls_api` watchlist: zero-argument `.get()`, `.orElseThrow()`

`Optional.get()` is a closer analogue to Rust's `.unwrap()` than TypeScript's
`!` was: it throws `NoSuchElementException` at the call site rather than
erasing at compile time.

Detection is nonetheless weaker than for either. tree-sitter gives syntax, not
types, and `foo.get()` is just as likely `Map.get` or `AtomicReference.get`.
The available filter is **zero arity** — `Map.get(k)` and `List.get(i)` always
take an argument, `Optional.get()` never does — plus `.orElseThrow()` and the
`OptionalInt`/`Long`/`Double` accessors. That narrows the false-positive set
substantially but not to zero; `Supplier.get()` and `ThreadLocal.get()` remain.

**The precision risk is known up front rather than suspected**, which is why
the rule ships at a lower maturity than TypeScript's did (see below).

## Rules shipped

Per the pack's audit → warn → block maturity policy, and joining `edited_file`
so they report the file in front of the user rather than the whole repository
on every edit:

| id | severity | fires on |
|---|---|---|
| `warn-import-cycle` | `warn` | package in an import cycle |
| `audit-untested-risky-call` | `audit` | production method using zero-arg `.get()` / `.orElseThrow()` with no direct test |

The severity split is deliberate. TypeScript shipped its risky-call rule at
`warn` with "demote if the corpus shows it noisy" as the fallback. Java's
precision problem is identified in advance, so the burden of proof runs the
other way: it ships at `audit` and is promoted to `warn` only if the corpus
supports it. The cycle rule has no precision problem and ships at `warn` as
usual.

## Out of scope

These are Java's real structural blind spots. They are recorded here so they
are known limits rather than corpus surprises.

- **Reflection and dependency injection.** Spring `@Autowired` by type,
  `Class.forName`, `ServiceLoader`. Java leans on these harder than Rust or
  Python do, and no import-based extractor sees them. This is the largest
  category of genuinely missing edges and it is not addressable at this tier.
- **Generated sources.** Lombok, MapStruct, annotation processors, Bazel
  `genrule` outputs. The code does not exist until build time. Bazel makes the
  gap more visible, since generated `srcs` are declared even when absent.
- **Fully-qualified inline usage** — `com.example.order.Order o = new
  com.example.order.Order();` uses no import statement and produces no edge.
  Uncommon in practice, and shared with every granularity choice.
- **Class-level coupling edges.** A future rule such as "this class has too
  many dependencies" would need class→class edges. The relation set is closed
  on purpose (the TypeScript spec rejected an intermediate relation for the
  same reason), so adding one is a deliberate event, not an increment.
- **Gradle.** `build.gradle` / `.kts` is a program, not a manifest, and the
  common declarative subset is far less stable than Bazel's. Deferred until
  there is a project that needs it.
- **Kotlin, Scala, and other JVM languages** sharing the same package model.
- **Promotion to `block`**, as for Rust, Python, and TypeScript.

## Testing

Unit tests per concern, following the Python and TypeScript extractors'
structure: identity from the `package` declaration, Maven source-root
detection (conventional and `<sourceDirectory>`-overridden), Bazel target
parsing (literal `srcs`, `glob()`, unparseable macro), a `java_library` and
`java_test` in one `BUILD` file yielding **one** unit while still classifying
their files differently, longest-prefix
resolution for each row of the resolution table, the package/path mismatch
incrementing `skipped`, unused-import suppression, `@Test` extraction, and
zero-arg `.get()` detection including the `Map.get(k)` negative case.

Integration tests through the real binary, extending
`tests/graph_structural_rules.rs`: a Maven project producing a graph, a Bazel
project producing a graph, a package cycle detected, both rules firing, and a
mixed Rust/Python/TypeScript/Java repository proving four languages coexist.

### Corpus validation is a merge gate

**Validation against real projects is required before merge, not optional.**
phronesis contains no Java, so there is no in-repo corpus. Synthetic fixtures
will pass while source-root detection quietly mis-fires on real layouts — the
failure mode this design is organised around.

Two corpora, one per discovery backend, since they share no code path:

- **Maven:** a multi-module, pure-Maven project — `apache/maven` itself is the
  leading candidate (unambiguously Maven, genuinely multi-module).
- **Bazel:** a Java tree built with Bazel — `bazelbuild/bazel` is the leading
  candidate, and its heavy macro use exercises the documented blind spot
  rather than hiding it.

Final selection is confirmed at implementation time by cloning each and
verifying its build files, not assumed from this document.

Each run must report: unit count, module count, `imports` edge count,
`skipped` count, and detected cycles — with the `skipped` count reconciled
against a human reading of the layout, and a sample of detected cycles
confirmed by hand. A `skipped` count that is not near-zero means source-root
detection is wrong, and the extractor does not merge until it is explained.

The risky-call rule's precision is measured on the same run and decides
whether it stays at `audit` or is promoted.

## Risks

1. **Source-root misdetection under Bazel.** Mitigated by the `skipped`
   counter, which is aimed directly at this symptom, and by the corpus gate.
   This is the highest-likelihood failure.
2. **Macro-defined Bazel targets leave files unclaimed.** They fall back to
   `java:project` and are counted. Acceptable for v1 and stated in Out of
   scope; a large fallback count on the corpus run is a signal to reconsider,
   not to ship quietly.
3. **Risky-call precision.** Known weak, which is why the rule ships at
   `audit`. If the corpus shows poor precision even there, ship the cycle rule
   alone — a rule nobody trusts is worse than no rule.
4. **Package-granular cycles read as over-reporting.** They are coarser by
   design (see Identity). Mitigated by rule wording that claims package
   interdependence rather than class-level circularity, and by hand-confirming
   a sample of corpus cycles.
5. **Discovery cost.** `UnitMap::discover` runs per save. Java adds a manifest
   walk and a declared-package set, but no file index and no file reads — it
   should be cheaper than TypeScript's contribution. Current per-save is
   6.5–10 ms. Measure rather than assume.

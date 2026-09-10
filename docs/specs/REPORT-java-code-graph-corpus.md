# Java code-graph corpus checkpoint

**Status: corpus accounting and graph-parity gate complete.** The omissions
below are reconciled; this does not establish compiler-equivalent classpath
coverage. Qwen's reassessment returned PASS after local verification of its
findings; see [the review report](REPORT-java-qwen-review.md).

## Reproduction

```sh
cargo run -p phronesis-mcp --example profile_java_graph -- /path/to/corpus
# Independently report grammar ERROR/missing nodes with source locations:
cargo run -p phronesis-mcp --example profile_java_graph -- /path/to/corpus --parse-errors
```

The reporter performs cold and warm discovery and requires byte-equivalent
edge vectors. Its JSON includes every named counter, diagnostic objects,
per-file skipped reasons, import provenance, and derived cycle membership.
Timings below are debug-build observations on the development machine.

Both repositories were cloned and their build files inspected:

- Apache Maven: `ea4a417bd2482448b8a5b2cf83f2738eee99d47f`, root `pom.xml`
  and module POMs; many nested POMs are deliberate integration-test fixtures.
- Bazel: `948b8c70e281c2fe42aa5468dad543c7af9f0ccc`, `MODULE.bazel`
  and Java package BUILD files, including macros and alias targets.

## Measurements

| Measure | Maven | Bazel |
|---|---:|---:|
| Java files | 3186 | 5827 |
| Units | 315 | 376 |
| Package modules | 558 | 458 |
| Skipped evidence items | 114 | 3856 |
| Cold discovery (ms) | 9547 | 20855 |
| Warm discovery (ms) | 5028 | 3618 |
| `imports` edges, per-file provenance | 3961 | 15769 |
| `defines_fn` edges, per-file provenance | 16207 | 62164 |
| `tested_by` edges, per-file provenance | 2364 | 11925 |
| Cycle components | 18 | 5 |

The latest call-resolution run supports direct constructed receivers,
including parenthesized qualified generic types, and prevents unresolved
inheritance from falling through to static imports. Compared with the prior
call-resolution checkpoint, Maven coverage increased from 2,335 to 2,364 edges
and Bazel coverage decreased from 12,137 to 11,925. Every other report field
except timings is unchanged, including import provenance, diagnostics, and
cycle membership. Both runs verified exact cold/warm edge equality. These
debug discovery timings are not end-to-end hook latency measurements; the
parser cache is process-local. The regression suite passed 751 graph tests,
and workspace Clippy with tests and examples passed at this checkpoint.

### Real post-check timing

Using a copied debug `phr-mcp` binary, the Maven corpus's full graph rebuild
took 31,256 ms and produced 27,513 base edges plus 63,675 derived edges. Two
subsequent, separate `post-check` processes took 23,243 ms and 30,240 ms.
Each received an Edit payload for an unchanged production Java file. Both
exited successfully and preserved the exact graph bytes. No post-phase
rules were configured, so these measurements exercise graph maintenance and
hook overhead without additional rule evaluation. Workspace integration
tests were also running during this initial measurement; it is not an
isolated benchmark or a release-build latency claim.

The Bazel debug rebuild took 79,446 ms (96,992 base edges and 243,377 derived
edges); separate hooks took 32,899 ms and 29,823 ms. The release binary was
then built with `cargo build --release -p phronesis-mcp --bin phr-mcp`, copied
to a stable temporary path, and measured sequentially with no concurrent
workspace build or test run:

| Release operation | Maven (ms) | Bazel (ms) |
|---|---:|---:|
| Full graph rebuild | 3915 | 7451 |
| Separate post-check process 1 | 3127 | 6997 |
| Separate post-check process 2 | 3263 | 10947 |

All six release commands exited 0. Both hooks on each corpus preserved
the exact graph bytes, and rebuild counts match the debug runs. The selected
files were Maven's
`api/maven-api-annotations/src/main/java/org/apache/maven/api/annotations/Config.java`
and Bazel's `examples/java-native/src/main/java/com/example/myproject/Greeter.java`.
These are unchanged-source hook measurements, not a claim that an actual
declaration edit costs the same. Two samples are insufficient for percentile
latency estimates.

This exposes substantial save-time cost even in release builds: a
process-local parser cache cannot accelerate successive one-shot hooks.
The post-check sensor currently performs a full graph rebuild; these timings
include extraction for other languages, derivation, persistence verification,
and hook bookkeeping as well as Java discovery. Stage profiling is needed
before attributing the whole delay to Java parsing or choosing a disk cache.

### Release CPU sampling

`/usr/bin/sample <pid> 10 1 -file <report>` sampled each live release
post-check process until it exited. Both sampled hooks exited 0. Inclusive
symbol counts from the main-thread call tree were:

| Sampled scope | Maven | Bazel |
|---|---:|---:|
| Main-thread samples | 2907 | 3876 |
| `Project::discover` | 1498 (51.5%) | 2399 (61.9%) |
| Java `parse::parse`, included in discovery | 793 (27.3%) | 1913 (49.4%) |
| Graph `persist` | 89 (3.1%) | 359 (9.3%) |

These are inclusive sampling proportions, not additive wall-clock stage
timings: parsing is already included in discovery, and sampling coverage
does not exactly equal elapsed milliseconds. Optimized/inlined code also
limits symbol attribution. Nonetheless both corpora identify discovery and
parsing as substantial costs; persistence alone is not the primary problem.
The next optimization experiment is a versioned, content-validated declaration
cache that survives hook processes, as anticipated in the design's
declaration-index cost risk. It must preserve the existing equal-size/mtime,
deletion, root-isolation, overlay, and exact graph-parity guarantees; a warm
cache result cannot substitute for checking changed inputs.

### Cross-process declaration cache experiment

The implemented cache uses `.phronesis/java-declarations.json`, a versioned,
repository-bound snapshot of parsed declarations and source hashes. It is
bounded to 64 MiB, atomically replaced, and disposable through `clean --cache`.
No build ownership or classpath result is cached. Six focused tests and a
real-binary regression passed, including disk round-trip reuse, changed/deleted
inputs, invalid versions/roots, corrupt/oversized files, symlink rejection,
same-size/same-mtime edits in a new hook process, and graph parity after cache
deletion or corruption.

The initial population measured 3,967 ms on Maven and 6,202 ms on Bazel.
Subsequent isolated release measurements, with no build/test processes running,
were:

| Warm disk cache operation | Maven (ms) | Bazel (ms) |
|---|---:|---:|
| Rebuild in a new process | 2230 | 2643 |
| Post-check process 1 | 2204 | 2746 |
| Post-check process 2 | 2217 | 2690 |
| Serialized cache bytes | 14097132 | 54514026 |

Both corpora's graph SHA-256 values match their pre-cache implementation
outputs exactly; each measured hook also preserved its graph bytes. Compared
with the earlier uncached release samples, this reduces Maven hook time by
roughly one third and Bazel time by more than half. Sample sizes are small,
and remaining full-rebuild work still makes these multi-second saves. The
optimization does not establish a low-latency incremental graph engine.

A follow-up Maven run after fixing build-helper execution inheritance
produced identical graph relations, import provenance, cycle membership,
skipped evidence, and diagnostics. That run measured 9,882 ms cold and
4,803 ms warm. The inheritance regression fixtures exercise execution-ID
replacement and inheritance controls that do not change this corpus's output.

After the bundled grammar fix, an isolated Bazel repeat produced identical
graph and diagnostic output (only timings changed); the reporter also
verified cold/warm edge parity. A Maven rerun retained every previous
graph/diagnostic field and measured 8,877 ms cold / 4,683 ms warm. Library
graph tests (745), library/example Clippy, formatting, and crate package-file
inspection passed for this change. These historical checks preceded the final
workspace validation recorded in the Qwen review report.

| Named counter | Maven | Bazel |
|---|---:|---:|
| `build_visibility_approximated` | 0 | 939 |
| `dependency_artifact_unsupported` | 97 | 0 |
| `dependency_identity_unresolved` | 2 | 0 |
| `dependency_modifier_ignored` | 136 | 0 |
| `dual_claimed_file` | 0 | 9 |
| `external_parent_skipped` | 76 | 0 |
| `files_unclaimed` | 0 | 884 |
| `group_id_unresolved` | 2 | 0 |
| `inactive_profile_skipped` | 174 | 1 |
| `jpms_unit_unmodelled` | 0 | 0 |
| `module_import_ignored` | 0 | 0 |
| `module_info_skipped` | 0 | 0 |
| `multi_release_root_ignored` | 0 | 0 |
| `multi_target_visibility_unioned` | 0 | 84 |
| `parent_cycle` | 1 | 0 |
| `select_branch_unioned` | 0 | 69 |
| `split_package` | 59 | 151 |
| `test_class_entry_point` | 0 | 108 |
| `test_class_unresolved` | 0 | 2 |
| `unbound_identifier` | 0 | 0 |
| `unit_id_collision` | 202 | 0 |
| `unit_identity_unresolved` | 4 | 0 |
| `unreadable_input` | 3 | 0 |
| `unresolved_label` | 0 | 1870 |
| `unresolved_property` | 6 | 0 |
| `unsupported_plugin_source_modifier` | 12 | 0 |
| `unsupported_syntax_skipped` | 0 | 55 |

## Manually checked cycle samples

Maven: `ParserRequest.java:33` imports
`org.apache.maven.api.cli.logging.AccumulatingLogger`, while
`logging/AccumulatingLogger.java:25` imports `org.apache.maven.api.cli.Logger`.
Both declarations belong to `api/maven-api-cli`, with unit
`org.apache.maven:maven-api-cli`. This confirms an `api.cli` / `api.cli.logging`
package cycle; it does not claim a cycle between those individual classes.

Bazel: `BazelJavaBuilder.java:23` imports `javac.plugins.BlazeJavaCompilerPlugin`,
and `BlazeJavaCompilerPlugin.java:17` imports `buildjar.InvalidCommandLineException`.
The `buildjar` target declares the plugins dependency. The plugins target
declares `:invalid_command_line_exception` in the buildjar package. These
source imports and target declarations confirm reciprocal package edges,
although the individual Bazel targets do not form that same cycle.

## Findings and remaining reconciliation

- Fixed: a non-UTF-8 Maven fixture initially aborted the snapshot. Unreadable
  inputs now remain explicit failed inputs while other files are analyzed.
- Fixed: Bazel BUILD files frequently sit below their Java source root.
  Recognizing ancestor roots reduced skipped items from 8,819 to 3,873.
- Fixed: the buildjar `:jarhelper` dependency is an `alias` with a literal
  `actual` target. Following it recovered two import edges. Conditional
  aliases now union string alternatives, including variable-bound selections.
  Seven formerly skipped selections are evaluated; import edges and cycle
  membership are unchanged. Alias label processing also now diagnoses 68
  external `@repository` targets previously dropped without an unresolved
  label diagnostic. These are external build dependencies, not evidence of
  68 missing in-repository Java imports.
- Maven has 202 colliding unit identities and a parent cycle among the
  discovered POMs. Many occur in integration-test fixture trees; do not
  equate the whole repository with one valid reactor. The full collision
  list still needs reconciliation before accepting the corpus gate.
- Bazel still has 884 unclaimed files, 55 unsupported statements, no Java
  parse failures, and substantial unresolved imports. Some unclaimed files
  are source fixtures referenced as data or live behind macros, but that
  The complete unclaimed-file partition is documented below; unresolved
  imports and parser failures remain separate open items.
- Reconciled 778 unclaimed files under `third_party/java/proguard`: its
  `BUILD:5` source filegroup packages the vendored tree, while `src/BUILD:495`
  consumes it in the `java_tools_dist` ZIP genrule. The package has no Java
  compilation target. These files correctly remain unclaimed under the
  source-container classification rule. The other 106 are reconciled below.
- Bazel's 798 counted package-path mismatches are reconciled below. No package
  names are synthesized from those paths; mismatches retain declarations.
- Reconciled all seven Maven parse failures: six `src/mdo/java/*.java` files
  are generation templates containing `package ${package};` (and in some
  cases template directives). The seventh is the intentional `mng-5208`
  `Bad.java` fixture containing `This is not a java file`.
- **Fixed grammar gap:** the original 14 Bazel parse failures occurred around
  qualified record patterns. For example, `BuildConfigurationValue.java:190`
  uses `case EnvVar.Set(String name, String value)` and
  `ConfigMatchingProvider.java:160` uses an `instanceof` pattern with
  `MatchResult.InError(...)`. A minimal reproduction parses `case Item(String
  value)` but reports an error for `case Qualified.Item(String value)`.
  [Record patterns are supported Java syntax](https://docs.oracle.com/en/java/javase/21/language/record-patterns.html).
  The pinned `tree-sitter-java` 0.23.5 `record_pattern` production accepts an
  identifier or generic type, but not a scoped non-generic type; the upstream
  [grammar](https://github.com/tree-sitter/tree-sitter-java/blob/master/grammar.js)
  inspected on 2026-09-05 has the same restriction. These were not malformed
  fixture files. The bundled grammar now accepts `scoped_type_identifier`
  in record patterns, eliminating all 14 failures without accepting partial
  syntax trees. This recovers 377 imports, 720 method definitions, and five
  `tested_by` edges. The 458 reported modules now reflect actual parsed
  packages; the previous 471 included artificial default-package owners
  assigned to failed inputs. The crate includes generated C, grammar source,
  headers, licenses, and regeneration instructions.
- Maven dependency-management/exclusion approximations and Bazel visibility
  and multi-target unions remain visible in counters. They can create or
  omit edges; a passing extraction command is not a precision claim.

## Bazel unclaimed-file reconciliation

All 884 files in the pinned corpus report fit this disjoint partition. This
accounts for source ownership only; it does not validate their imports or
make the overall corpus gate pass.

| Files | Classification | BUILD/source evidence |
|---:|---|---|
| 778 | Vendored ProGuard source archive | `third_party/java/proguard/BUILD:5` filegroup; `src/BUILD:495` source ZIP genrule |
| 84 | Annotation-processor resource inputs | 8 Starlark configuration-field inputs, 43 options-processor inputs, 33 Starlark annotation inputs; their tests use `resources`, not compilation `srcs` |
| 9 | ijar runtime data inputs | `third_party/ijar/test/BUILD` lists the Java files in `ijar_test` or `IjarTests` `data` |
| 3 | Sources accompanying precompiled ijar fixtures | `nestmates/NestTest.java`, `records/RecordTest.java`, `sealed/SealedTest.java`; BUILD consumes the corresponding committed JARs |
| 3 | Explicitly excluded JarJar adapters | `third_party/jarjar/BUILD:30` excludes `AntJarProcessor.java`, `JarJarMojo.java`, and `JarJarTask.java` |
| 2 | Sources for temporary runfiles projects | `runfiles_test.py:23` copies `Foo.java`, `Bar.java`, and `BUILD.mock` files into a test workspace |
| 5 | No matching source declaration in the owning BUILD | Files listed individually below; owning packages compile explicit source lists or unrelated globs |
| **884** | **Total** | **No unassigned remainder** |

The processor inputs are under
`src/test/java/com/google/devtools/build/lib/analysis/starlark/annotations/processor/optiontestsources`,
`src/test/java/com/google/devtools/common/options/processor/optiontestsources`,
and `src/test/java/net/starlark/java/annot/processor/testsources`.
Their enclosing BUILD files declare resource inputs at lines 17, 17/31, and
17 respectively. A filegroup used as a resource does not establish Java
compilation ownership.

The nine ijar data inputs are `A.java`, `B.java`, `WellCompressed1.java`,
`WellCompressed2.java`, `TypeAnnotationTest2.java`, `PrivateNestedClass.java`,
`UseDeprecatedParts.java`, `UseRestrictedAnnotation.java`, and
`package-info.java`. BUILD comments identify the precompiled fixture JARs;
the corresponding source files have no compilation target in that BUILD.

The five unmatched sources are `ImportDepsCheckerTest.java` and
`ResolutionFailureChainTest.java` in the import-deps checker test package,
its `testdata/JspecifyUser.java`, the shell package's
`WindowsSubprocessFactoryTest.java`, and the Starlark eval package's
`CpuProfilerTest.java`. Each is absent from its owning BUILD's explicit
compilation source lists. Broad source-archive filegroups include them but
do not supply a compilation context. This describes the pinned build files;
it does not infer why these files were left out or whether another build
system uses them.

## Bazel package-path mismatch reconciliation

There are 798 counted mismatches among successfully parsed sources. The
previous 14 additional flags on parse-failed inputs disappeared after the
record-pattern grammar fix. All 798 remaining mismatches fall into these groups:

| Files | Layout evidence |
|---:|---|
| 778 | The ProGuard archive sits beneath `third_party/java/proguard/proguard6.2.2`; the conventional ancestor `java` directory is not its actual embedded source root. These archive-only files retain their declared packages. |
| 4 | Import-deps checker fixtures: default-package `JspecifyUser.java`, `j_p_l/A.java`, and `j_p_l/B.java`, plus a replacement `Library.java` nested under `library_no_members` with the original library's declared package. |
| 2 | Default-package `Foo.java` and `Bar.java` are copied into temporary runfiles projects. |
| 1 | `src/tools/diskcache/Gc.java` declares `diskcache`; its BUILD explicitly names `diskcache.Gc` as the binary main class. The containing Bazel package is not a Java source root. |
| 13 | ijar layouts: five generator/utility classes declare package `test` directly in the BUILD directory; `Object.java` declares `java.lang`; seven fixture classes in nested directories use the default package. |
| **798** | **Total, with no unassigned remainder** |

The five ijar classes declaring `test` are
`GenDuplicatedUnsupportedAttribute`, `GenDynamicConstant`,
`GenSourceDebugExtension`, `GenZipWithEntries`, and `ZipCount`.
The seven nested default-package fixtures are `invokedynamic/ClassWithLambda`,
`nestmates/NestTest`, `records/RecordTest`, `sealed/SealedTest`, and
`typeannotations2/{NonNull,Nullable,Util}`. These inspected package declarations
explain the diagnostic; they do not justify deriving package names from paths
or dropping these sources.

This is a limitation of the conventional source-root cross-check on Bazel
layouts, not evidence of 798 malformed Java packages or 798 missing graph
modules. The unresolved-import partition is documented below. These path diagnostics
do not independently establish import coverage.

## Bazel unresolved-import reconciliation

The remaining 3,058 unresolved imports all originate in files without a
compilation claim. Their declarations are indexed, but no owning compilation
target supplies a classpath, so visibility filtering deliberately cannot
resolve these imports. They partition as follows:

| Imports | Source group | Ownership evidence |
|---:|---|---|
| 2,677 | Vendored ProGuard archive | Source packaging only |
| 377 | Annotation-processor resource fixtures | Loaded through resources, not target `srcs` |
| 2 | `JarJarMojo.java` and `JarJarTask.java` | Explicitly excluded adapters |
| 2 | `CpuProfilerTest.java` | Absent from the owning compilation source list |
| **3,058** | **Total** | **Every source is in the reconciled unclaimed set** |

One previous unresolved import was a resolver defect:
`CollectPackagesUnderDirectoryFunction.java` imports the nested
`ProcessPackageDirectorySkyFunctionException`. Its BUILD dependency is
`"process_package_directory"`, without a colon. Bazel permits both bare and
colon-prefixed same-package labels. Supporting this form resolves the import
to its own package, where self-edge suppression applies, so the import-edge
count does not increase. Six previously unresolved label diagnostics also
disappear. Regression coverage includes bare sources/filegroups, dependencies,
exports, and aliases.

The current corpus run has no unresolved imports originating in claimed
compilation sources. This statement covers imports whose declaration owners
exist in the index; imports classified as external or absent/generated types
still require the documented limits on declaration-based resolution. It is
not a claim of complete compiler-equivalent dependency resolution.

## Maven fixture identities and unresolved imports

All 202 duplicate unit identities occur beneath `src/test` trees: 161 in
`its/core-it-suite`, 37 in `impl/maven-core`, two in `compat/maven-compat`,
one in `compat/maven-model`, and one in `impl/maven-impl`. These are independent
test POMs reusing coordinates, not 202 collisions among production reactor
modules. The parent cycle is in `mng-11009-stackoverflow-parent-resolution`;
its parent POM explicitly documents the circular relative-path setup.
The duplicate-identity policy can leave some fixture sources on fallback
ownership, so these fixtures must not be treated as fully resolved projects.

The reporter now includes source unit/context and
`unresolved_visibility_candidates` for every unresolved import. These
candidates come directly from the resolver's visibility callback, after its
production/test filter. All 78 Maven unresolved imports have no visible
candidate; none is a choice between multiple visible declarations.

| Imports | Type family | Inspected cause or limitation |
|---:|---|---|
| 36 | Plexus XML (`Xpp3Dom`, `MXSerializer`, `XmlSerializer`) | Indexed copies belong to the `maven-it-plugin-plexus-utils-new` shading fixture; importing modules do not depend on that plugin. Their external Plexus artifacts are not indexed. |
| 19 | Legacy `MavenProject`, `MavenReportException`, `ProjectDependenciesResolver` | Current declarations live in core/compat units, while old fixture/support POMs refer to legacy project/reporting/core artifacts or have fallback ownership. Name equality does not establish a dependency on the current declaration owner. |
| 12 | Maven 4 API types | Six occur in a fixture with no recognized local POM (`mng-12534`); six in `mng-8220` extensions declaring the former `maven-api-impl` artifact, which does not grant visibility to the current core/SPI owners. |
| 5 | Legacy plugin API types | Sources with fallback ownership in the `mng-3536`/`mng-3740` fixture layouts do not receive the plugin API unit's classpath. |
| 4 | `CompileOnlyDep` / `TestOnlyDep` | The `mng-8750-new-scopes` fixtures exercise Maven 4 `compile-only` and `test-only`; the implemented classic scope table diagnoses these as unsupported. This remains a scope-support limitation. |
| 2 | Artifact factory types | `mng-2771` declares the legacy `maven-artifact` dependency; these classes' current indexed owner is `maven-core`. |
| **78** | **Total** | **No visible declaration candidate in any case** |

This explains the observed visibility decisions; it does not claim external
artifact resolution, support for all Maven 4 scopes, or correct compilation
of fixture projects intentionally testing older Maven layouts. The 29 counted
path mismatches are partitioned below. Together with seven parse failures,
these account for all 114 skipped evidence items.

## Maven package-path mismatch reconciliation

| Files | Inspected reason |
|---:|---|
| 15 | Owning fixture POM has a duplicate unit identity and is excluded by the deterministic collision policy. These files use fallback ownership: six `cyclic-import-scope` v2/v3 files, one each in MNG-3038/MNG-3740/MNG-5640, three in MNG-4660, and three in MNG-5760. |
| 3 | The nearest POM uses `packaging=pom`, which defines no Java unit: the MNG-3536 fixture source, `JvmConfigParser.java` in the Maven distribution, and root `src/graph/ReactorGraph.java`. The launcher explicitly runs `JvmConfigParser.java` in Java source-launch mode; `ReactorGraph.java` is a JBang script. |
| 2 | The MNG-12534 plugin source fixture has no local `pom.xml`; its nearest POM is the enclosing integration-test suite. |
| 3 | Sources lie outside configured/conventional roots: MNG-5581 uses `main/java` and `test/java` with a lifecycle extension, while MNG-7587 places `MyMojoTest.java` directly in `src/test`. |
| 6 | Declared packages differ from paths: IT0030's `Person` declares `it0001`; both MNG-2054 files add an `it0096` component; MNG-6240's `TestMojo` declares an MNG-6209 package; two MNG-8750 dependency classes declare an additional `mng8750` component. |
| **29** | **Total, with no unassigned remainder** |

The 23 fallback-owned files and six normally owned files retain their actual
package declarations. These observations explain the cross-check output;
they do not claim that arbitrary lifecycle extensions or duplicate fixture
projects receive complete build-model resolution.

## Validation scope

Final review validation passes 2,487 workspace Rust tests, 46 BDD scenarios,
workspace Clippy, build, and formatting. Corpus reruns preserve complete graph
bytes after the module-import fix and regenerate caches at format 2. Coverage
totals reflect corrected lexical/import precedence, direct constructed
receivers, and conservative handling of ambiguous types, overloads, and
unresolved inherited methods. Earlier cold/warm comparisons also preserve
edge equality. Ordinary variable receivers and inherited dispatch are not
resolved; coverage completeness is not asserted.

The four-language real-binary integration test also passes: a Maven manifest
rename through `post-check` refreshes Java coverage identities, preserves the
Rust/Python/TypeScript source edges, and yields exactly the graph written by
a subsequent clean CLI rebuild. Targeted test Clippy and formatting pass.

The implementation has passed 36 Java component tests, 57 graph-sync tests,
two real-binary Java hook tests, formatting, and workspace Clippy. A full
workspace test run passed with socket access before the final literal-alias
change; that change passed its focused regression test. Final gates must
be rerun after the remaining fixes.

## Published grammar dependency — 2026-09-09

The bundled parser described in the historical measurements above is replaced
by the published `tree-sitter-java-orchard` dependency. A maintainer identifies
this fork in the original [qualified-pattern PR discussion](https://github.com/tree-sitter/tree-sitter-java/pull/231).
The repository no longer carries the generated Java C parser, grammar source,
headers, custom build script, or unsafe language loader.

The dependency is pinned to **0.5.8**, rather than the latest 0.5.15. Direct
probes found that 0.5.9, 0.5.10, 0.5.11, 0.5.12, and 0.5.15 reject a Kelvin
sign inside a string (`class A { String s = "K"; }`). The 0.5.15 corpus run
therefore rejected `FileSystemTest.java:2089` and
`WindowsFileSystemTest.java:492`, losing 112 import edges and 13 coverage
edges. Version 0.5.8 parses both complete files and qualified record patterns.
`unicode_string_literals_preserve_complete_file_evidence` locks in that
requirement for future dependency updates.

Orchard uses named `modifier` nodes for `static`; the extractor now reads
those nodes. Module-import validation explicitly rejects reserved names and
keeps ordinary imports whose type or package is named `module`. Existing
qualified-pattern, malformed-input, and coverage regressions remain enabled.
Declaration-cache format 3 invalidates parses produced before these changes.

The final **0.5.8** runs used the same pinned commits above and matched the
fresh bundled-parser baseline in every non-timing JSON field: import edges
with provenance, cycle membership, relation counts, diagnostics, failed inputs,
and per-file skipped details. Both reporters also verified cold/warm equality
of their complete extracted edge vectors. This comparison does not establish
compiler-equivalent resolution or cross-parser equality of every non-import
edge; the report serializes counts for those relations.

| Measure | Maven | Bazel |
|---|---:|---:|
| Files | 3186 | 5827 |
| Imports | 3961 | 15769 |
| Method definitions | 16207 | 62164 |
| `tested_by` | 2364 | 11925 |
| Skipped evidence items | 114 | 3856 |
| Parse-failed inputs | 7 | 0 |
| Cold discovery (ms) | 5584 | 18260 |
| Warm discovery (ms) | 1218 | 2270 |

Concurrent debug-build timings are observations, not an isolated benchmark.
The seven Maven failures remain the previously reconciled templates and
intentionally invalid fixture.

The same remediation bounds eager BUILD value expansion and shares Bazel
visibility sets among identical claiming-target sets. A synthetic single
3,000-file target used approximately 29 MB peak child RSS instead of the
handover's reported 1.34 GB rebuild peak; the operations differ, so this is
scaling evidence rather than a like-for-like speedup claim. Doubling-list
fixtures at 20 and 40 assignments each produced one budget diagnostic at
approximately 28 MB peak child RSS. Literal 10,000-entry source lists remain
supported. Enhanced-for variables now suppress false static-receiver coverage,
with parser and project regressions.

Final verification on Orchard 0.5.8: `cargo test --workspace` passed 2,499
Rust tests (one ignored), plus 46 BDD scenarios / 181 steps. Workspace/example
builds, `cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo fmt --all -- --check` exited 0. The local graph rebuild exited 0.
